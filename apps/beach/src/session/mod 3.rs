pub mod client;
pub mod message_handlers;
pub mod signaling_transport;

use url::{Url, ParseError};
use crate::transport::Transport;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use anyhow::Result;
use crate::config::Config;
use self::signaling_transport::{SignalingTransport, create_websocket_signaling};
use crate::transport::websocket::WebSocketTransport;
use crate::protocol::signaling::{AppMessage, ClientMessage, ServerMessage, PeerRole, TransportSignal, WebRTCSignal};
use self::message_handlers::{ServerMessageHandler, ClientMessageHandler};
use crate::transport::webrtc::remote_signaling::RemoteSignalingChannel;

#[derive(Debug, Clone)]
pub struct SessionUrl(Url);

impl SessionUrl {
    pub fn new(session_id: &str) -> Self {
        Self(Url::parse(&format!("https://{}", session_id)).unwrap())
    }

    pub fn parse(s: &str) -> Result<Self, ParseError> {
        let url = if s.contains("://") {
            Url::parse(s)?
        } else {
            Url::parse(&format!("https://{}", s))?
        };
        Ok(Self(url))
    }

    pub fn as_url(&self) -> &Url {
        &self.0
    }
}

pub(crate) fn generate_session_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub struct Session<T: Transport + Send + 'static> {
    id: String,
    url: SessionUrl,
    transport: Arc<T>,
    passphrase: Option<String>,
}

pub struct ServerSession<T: Transport + Send + 'static> {
    session: Session<T>,
    cmd: Vec<String>,
    clients: Arc<RwLock<HashMap<String, bool>>>, // client id, connected
    signaling: Option<Arc<SignalingTransport<WebSocketTransport>>>,
    handler: Option<Arc<dyn ServerMessageHandler>>,
    debug_handler: Option<Arc<crate::server::debug_handler::DebugHandler>>,
    /// WebRTC signaling channels per peer
    webrtc_channels: Arc<RwLock<HashMap<String, Arc<RemoteSignalingChannel>>>>,
}

impl<T: Transport + Send + 'static> ServerSession<T> {
    pub fn new(session: Session<T>, cmd: Vec<String>) -> Self {
        Self {
            session,
            cmd,
            clients: Arc::new(RwLock::new(HashMap::new())),
            signaling: None,
            handler: None,
            debug_handler: None,
            webrtc_channels: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Connect to session server WebSocket
    pub async fn connect_signaling(&mut self, session_server: &str, session_id: &str) -> Result<()> {
        let connection = create_websocket_signaling(
            session_server,
            session_id,
            self.session.id.clone(),
            self.session.passphrase.clone(),
            PeerRole::Server,
        ).await?;
        
        self.signaling = Some(Arc::new(connection));
        
        // Start message router
        if let Some(handler) = &self.handler {
            self.start_message_router(handler.clone()).await;
        }
        
        Ok(())
    }

    /// Set the message handler but don't start the router yet
    pub async fn set_handler(&mut self, handler: Arc<dyn ServerMessageHandler>) {
        self.handler = Some(handler.clone());
        // Don't start the router yet - wait for start_router() to be called explicitly
    }
    
    /// Set the debug handler
    pub fn set_debug_handler(&mut self, debug_handler: Arc<crate::server::debug_handler::DebugHandler>) {
        self.debug_handler = Some(debug_handler);
    }
    
    /// Start the message router after all handlers have been set
    pub async fn start_router(&self) {
        if let (Some(_), Some(handler)) = (&self.signaling, &self.handler) {
            self.start_message_router(handler.clone()).await;
        }
    }

    /// Start routing messages from WebSocket to handler
    async fn start_message_router(&self, handler: Arc<dyn ServerMessageHandler>) {
        if let Some(signaling) = &self.signaling {
            let signaling = signaling.clone();
            let signaling_sender = signaling.clone();
            let clients = self.clients.clone();
            let debug_handler = self.debug_handler.clone();
            let webrtc_channels = self.webrtc_channels.clone();
            let transport = self.session.transport.clone();
            
            tokio::spawn(async move {
                while let Some(msg) = signaling.recv().await {
                    match msg {
                        ServerMessage::PeerJoined { peer } => {
                            clients.write().await.insert(peer.id.clone(), true);
                            handler.handle_client_joined(&peer).await;
                            
                            // Create WebRTC signaling channel for this peer
                            let channel = RemoteSignalingChannel::new(
                                signaling_sender.clone(),
                                peer.id.clone(),
                            );
                            channel.set_remote_peer(peer.id.clone()).await;
                            
                            // Store the channel as Arc
                            let channel_arc = Arc::new(channel);
                            webrtc_channels.write().await.insert(peer.id.clone(), channel_arc.clone());
                            
                            // Initiate WebRTC connection if transport supports it
                            if transport.is_webrtc() {
                                let transport_clone = transport.clone();
                                let channel_any = channel_arc as Arc<dyn std::any::Any + Send + Sync>;
                                
                                // Spawn the WebRTC connection initiation with timeout
                                tokio::spawn(async move {
                                    let timeout_duration = std::time::Duration::from_secs(30);
                                    let result = tokio::time::timeout(
                                        timeout_duration,
                                        transport_clone.initiate_webrtc_with_signaling(
                                            channel_any,
                                            true // Server is the offerer
                                        )
                                    ).await;
                                    
                                    match result {
                                        Ok(Ok(())) => {
                                            // WebRTC transport will log internally
                                        }
                                        Ok(Err(e)) => {
                                            if std::env::var("BEACH_STRICT_WEBRTC").is_ok() {
                                                panic!("WebRTC connection required but failed: {}. Ensure both peers support WebRTC and network allows peer-to-peer connections.", e);
                                            }
                                        }
                                        Err(_) => {
                                            if std::env::var("BEACH_STRICT_WEBRTC").is_ok() {
                                                panic!("WebRTC handshake timed out after 30 seconds. This may indicate: 1) Network firewall blocking WebRTC, 2) Client not responding, 3) STUN/TURN servers unreachable");
                                            }
                                        }
                                    }
                                });
                            }
                        }
                        ServerMessage::PeerLeft { peer_id } => {
                            clients.write().await.remove(&peer_id);
                            webrtc_channels.write().await.remove(&peer_id);
                            handler.handle_client_left(&peer_id).await;
                        }
                        ServerMessage::Debug { response } => {
                            // This shouldn't happen on the server side (Debug is sent TO us, not FROM signaling)
                            // But just in case...
                        }
                        ServerMessage::Signal { from_peer, signal } => {
                            // Check if this is a WebRTC signal
                            if let Ok(transport_signal) = TransportSignal::from_value(&signal) {
                                if let TransportSignal::WebRTC { .. } = transport_signal {
                                    // Handle WebRTC signaling
                                    if let Some(channel) = webrtc_channels.read().await.get(&from_peer) {
                                        let _ = channel.handle_signal(signal.clone()).await;
                                    }
                                    continue;
                                }
                            }
                            
                            // Check if this is a debug request (Custom transport signal)
                            if let Some(transport) = signal.get("transport").and_then(|v| v.as_str()) {
                                if transport == "custom" {
                                    if let (Some(transport_name), Some(signal_type)) = 
                                        (signal.get("transport_name").and_then(|v| v.as_str()),
                                         signal.get("signal_type").and_then(|v| v.as_str())) {
                                        
                                        if transport_name == "debug" && signal_type == "debug_request" {

                                            // Handle debug request
                                            if let (Some(debug_handler), Some(payload)) = 
                                                (debug_handler.as_ref(), signal.get("payload")) {
                                                
                                                if let Some(request) = payload.get("request") {
                                                    // Process the debug request
                                                    let response = debug_handler.handle_debug_request(request.clone()).await;
                                                    
                                                    // Send response back via signaling as Custom transport signal
                                                    let debug_response = serde_json::json!({
                                                        "transport": "custom",
                                                        "transport_name": "debug",
                                                        "signal_type": "debug_response",
                                                        "payload": {
                                                            "response": response,
                                                        }
                                                    });
                                                    
                                                    let msg = ClientMessage::Signal {
                                                        to_peer: from_peer.clone(),
                                                        signal: debug_response,
                                                    };
                                                    let _ = signaling_sender.send(msg).await;
                                                }
                                            }
                                            continue;
                                        } else {
                                        }
                                    } else {
                                    }
                                } else {
                                }
                            } else {
                            }
                            
                            // Otherwise, try to parse as app message
                            if let Ok(app_msg) = serde_json::from_value::<AppMessage>(signal) {
                                handler.handle_client_message(&from_peer, app_msg).await;
                            }
                        }
                        _ => {}
                    }
                }
            });
        }
    }

    /// Broadcast message to all clients
    pub async fn broadcast_to_clients(&self, message: AppMessage) -> Result<()> {
        // Check if strict WebRTC mode is enabled
        let strict_webrtc = std::env::var("BEACH_STRICT_WEBRTC").is_ok();
        
        let clients = self.clients.read().await;
        
        // In strict mode, only use WebRTC for app data
        if strict_webrtc {
            if self.session.transport.is_webrtc() && self.session.transport.is_connected() {
                let bytes = serde_json::to_vec(&message)?;
                // In broadcast, we send to all connected clients via the transport
                // Note: This assumes the transport handles broadcasting internally
                return self.session.transport.send(&bytes).await
                    .map_err(|e| anyhow::anyhow!("WebRTC broadcast failed: {}", e));
            } else {
                return Err(anyhow::anyhow!("WebRTC required but not connected for broadcast"));
            }
        }
        
        // Non-strict mode: Try WebRTC first, fall back to WebSocket
        if self.session.transport.is_webrtc() && self.session.transport.is_connected() {
            let bytes = serde_json::to_vec(&message)?;
            if self.session.transport.send(&bytes).await.is_ok() {
                return Ok(());
            }
        }
        
        // WebSocket fallback (only in non-strict mode)
        if let Some(signaling) = &self.signaling {
            for (client_id, _) in clients.iter() {
                let msg = ClientMessage::Signal {
                    to_peer: client_id.clone(),
                    signal: serde_json::to_value(&message)?,
                };
                signaling.send(msg).await?;
            }
        }
        Ok(())
    }

    /// Send message to specific client
    pub async fn send_to_client(&self, client_id: &str, message: AppMessage) -> Result<()> {
        // Check if strict WebRTC mode is enabled
        let strict_webrtc = std::env::var("BEACH_STRICT_WEBRTC").is_ok();
        
        // In strict mode, only use WebRTC for app data
        if strict_webrtc {
            // Check if we have a WebRTC connection for this client
            if self.session.transport.is_webrtc() && self.session.transport.is_connected() {
                let bytes = serde_json::to_vec(&message)?;
                return self.session.transport.send(&bytes).await
                    .map_err(|e| anyhow::anyhow!("WebRTC send to client {} failed: {}", client_id, e));
            } else {
                return Err(anyhow::anyhow!("WebRTC required but not connected for client {}", client_id));
            }
        }
        
        // Non-strict mode: Try WebRTC first, fall back to WebSocket
        if self.session.transport.is_webrtc() && self.session.transport.is_connected() {
            let bytes = serde_json::to_vec(&message)?;
            if self.session.transport.send(&bytes).await.is_ok() {
                return Ok(());
            }
        }
        
        // WebSocket fallback (only in non-strict mode)
        if let Some(signaling) = &self.signaling {
            let msg = ClientMessage::Signal {
                to_peer: client_id.to_string(),
                signal: serde_json::to_value(&message)?,
            };
            signaling.send(msg).await?;
        }
        Ok(())
    }

    pub fn session(&self) -> &Session<T> {
        &self.session
    }

    pub fn cmd(&self) -> &Vec<String> {
        &self.cmd
    }

    /// Check if any clients are connected
    pub async fn has_clients(&self) -> bool {
        let clients = self.clients.read().await;
        !clients.is_empty()
    }

    /// Check if any WebRTC connection is established
    pub async fn has_any_webrtc_connected(&self) -> bool {
        // Check if the transport itself is WebRTC and connected
        if self.session.transport.is_webrtc() && self.session.transport.is_connected() {
            return true;
        }
        
        // Also check webrtc_channels for any connected channels
        let channels = self.webrtc_channels.read().await;
        for (_peer_id, channel) in channels.iter() {
            // Check if channel has received an offer/answer (indicating connection progress)
            // Note: We can't directly check channel connection state from here,
            // but having a channel means WebRTC negotiation is in progress
            if channels.len() > 0 {
                return true;
            }
        }
        
        false
    }
}

pub struct ClientSession<T: Transport + Send + 'static> {
    session: Session<T>,
    client_instance_id: String,
    signaling: Option<Arc<SignalingTransport<WebSocketTransport>>>,
    handler: Option<Arc<dyn ClientMessageHandler>>,
    server_peer_id: Option<String>,
    /// WebRTC signaling channel for server
    webrtc_channel: Option<Arc<RemoteSignalingChannel>>,
}

impl<T: Transport + Send + 'static> ClientSession<T> {
    pub fn new(session: Session<T>) -> Self {
        Self {
            session,
            client_instance_id: generate_session_id(),
            signaling: None,
            handler: None,
            server_peer_id: None,
            webrtc_channel: None,
        }
    }

    /// Connect to session server WebSocket
    pub async fn connect_signaling(&mut self, session_server: &str, session_id: &str) -> Result<()> {
        let connection = create_websocket_signaling(
            session_server,
            session_id,
            self.client_instance_id.clone(),
            self.session.passphrase.clone(),
            PeerRole::Client,
        ).await?;
        
        self.signaling = Some(Arc::new(connection));
        
        // Start message router
        if let Some(handler) = &self.handler {
            self.start_message_router(handler.clone()).await;
        }
        
        Ok(())
    }

    /// Set the message handler
    pub fn set_handler(&mut self, handler: Arc<dyn ClientMessageHandler>) {
        self.handler = Some(handler);
    }

    /// Start routing messages from WebSocket to handler
    async fn start_message_router(&self, handler: Arc<dyn ClientMessageHandler>) {
        if let Some(signaling) = &self.signaling {
            let signaling = signaling.clone();
            let signaling_sender = signaling.clone();
            let transport = self.session.transport.clone();
            
            tokio::spawn(async move {
                let mut server_peer_id: Option<String> = None;
                let mut webrtc_channel: Option<Arc<RemoteSignalingChannel>> = None;
                
                while let Some(msg) = signaling.recv().await {
                    match msg {
                        ServerMessage::JoinSuccess { peers, .. } => {
                            // Find the server peer
                            for peer in &peers {
                                if matches!(peer.role, PeerRole::Server) {
                                    server_peer_id = Some(peer.id.clone());
                                    
                                    // Create WebRTC signaling channel for server
                                    let channel = RemoteSignalingChannel::new(
                                        signaling_sender.clone(),
                                        peer.id.clone(),
                                    );
                                    channel.set_remote_peer(peer.id.clone()).await;
                                    
                                    // Store the channel as Arc for both spawned task and local usage
                                    let channel_arc = Arc::new(channel);
                                    webrtc_channel = Some(channel_arc.clone());
                                    
                                    // Don't initiate WebRTC here - wait for server's Offer
                                    // WebRTC transport will log internally when ready
                                    
                                    break;
                                }
                            }
                        }
                        ServerMessage::PeerJoined { peer } => {
                            handler.handle_peer_joined(&peer).await;
                        }
                        ServerMessage::PeerLeft { peer_id } => {
                            handler.handle_peer_left(&peer_id).await;
                        }
                        ServerMessage::Signal { from_peer, signal } => {
                            // Check if this is a WebRTC signal
                            if let Ok(transport_signal) = TransportSignal::from_value(&signal) {
                                if let TransportSignal::WebRTC { signal: webrtc_signal } = transport_signal {
                                    // If this is an Offer and we haven't initiated yet, do it now
                                    if matches!(webrtc_signal, WebRTCSignal::Offer { .. }) {
                                        if transport.is_webrtc() && webrtc_channel.is_some() {
                                            let transport_clone = transport.clone();
                                            let channel_arc = webrtc_channel.as_ref().unwrap().clone();
                                            let channel_any = channel_arc.clone() as Arc<dyn std::any::Any + Send + Sync>;
                                            
                                            // First, handle the signal to store it
                                            let _ = channel_arc.handle_signal(signal.clone()).await;
                                            
                                            // Then initiate WebRTC as answerer with timeout
                                            tokio::spawn(async move {
                                                let timeout_duration = std::time::Duration::from_secs(30);
                                                let result = tokio::time::timeout(
                                                    timeout_duration,
                                                    transport_clone.initiate_webrtc_with_signaling(
                                                        channel_any,
                                                        false // Client is the answerer
                                                    )
                                                ).await;
                                                
                                                match result {
                                                    Ok(Ok(())) => {
                                                        // WebRTC transport will log internally
                                                    }
                                                    Ok(Err(e)) => {
                                                        if std::env::var("BEACH_STRICT_WEBRTC").is_ok() {
                                                            panic!("WebRTC connection required but failed: {}. Ensure both peers support WebRTC and network allows peer-to-peer connections.", e);
                                                        }
                                                    }
                                                    Err(_) => {
                                                        if std::env::var("BEACH_STRICT_WEBRTC").is_ok() {
                                                            panic!("WebRTC handshake timed out after 30 seconds. This may indicate: 1) Network firewall blocking WebRTC, 2) Server not responding, 3) STUN/TURN servers unreachable");
                                                        }
                                                    }
                                                }
                                            });
                                        }
                                    } else {
                                        // Handle other WebRTC signals normally
                                        if let Some(ref channel) = webrtc_channel {
                                            let _ = channel.handle_signal(signal.clone()).await;
                                        }
                                    }
                                    continue;
                                }
                            }
                            
                            // Otherwise try to parse as app message
                            if let Ok(app_msg) = serde_json::from_value::<AppMessage>(signal) {
                                handler.handle_server_message(app_msg).await;
                            }
                        }
                        _ => {}
                    }
                }
            });
        }
    }

    /// Send message to server
    pub async fn send_to_server(&self, message: AppMessage) -> Result<()> {
        // Check if strict WebRTC mode is enabled
        let strict_webrtc = std::env::var("BEACH_STRICT_WEBRTC").is_ok();
        
        // In strict mode, only use WebRTC for app data
        if strict_webrtc {
            if !self.session.transport.is_connected() {
                return Err(anyhow::anyhow!("WebRTC required but not connected"));
            }
            let bytes = serde_json::to_vec(&message)?;
            return self.session.transport.send(&bytes).await
                .map_err(|e| anyhow::anyhow!("WebRTC send failed: {}", e));
        }
        
        // Non-strict mode: Try WebRTC first, fall back to WebSocket
        if self.session.transport.is_webrtc() && self.session.transport.is_connected() {
            let serialized = serde_json::to_vec(&message)?;
            if self.session.transport.send(&serialized).await.is_ok() {
                return Ok(());
            }
        }
        
        // WebSocket fallback (only in non-strict mode)
        if let Some(signaling) = &self.signaling {
            if let Some(server_id) = &self.server_peer_id {
                let msg = ClientMessage::Signal {
                    to_peer: server_id.clone(),
                    signal: serde_json::to_value(&message)?,
                };
                signaling.send(msg).await?;
            }
        } else {
            return Err(anyhow::anyhow!("No transport available to send message"));
        }
        Ok(())
    }

    pub fn session(&self) -> &Session<T> {
        &self.session
    }

    pub fn client_instance_id(&self) -> &str {
        &self.client_instance_id
    }
}

impl<T: Transport + Send + 'static> Session<T> {
    pub fn new(url: SessionUrl, transport: T, passphrase: Option<String>) -> Self {
        Self { url, transport: Arc::new(transport), passphrase, id: generate_session_id() }
    }

    /// Create a new session and register it with the session server
    pub async fn create(
        config: &Config, 
        transport: T, 
        passphrase: Option<String>, 
        cmd: Vec<String>
    ) -> Result<ServerSession<T>> {
        // Generate a session ID
        let session_id = generate_session_id();
        
        // Register session with session server
        let session_client = client::SessionClient::new(&config.session_server);
        
        // Try to register with a timeout to avoid hanging forever
        let registration = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            session_client.register_session(&session_id, passphrase.as_deref())
        ).await;
        
        let session_url_str = match registration {
            Ok(Ok(url)) => {
                eprintln!("✅ Successfully registered with session server");
                url
            },
            Ok(Err(e)) => {
                eprintln!("⚠️  Failed to register with session server: {}", e);
                eprintln!("⚠️  Continuing in local mode (clients cannot connect remotely)");
                format!("{}/{}", config.session_server, session_id)
            },
            Err(_) => {
                eprintln!("⚠️  Session server registration timed out after 2 seconds");
                eprintln!("⚠️  Continuing in local mode (clients cannot connect remotely)");
                format!("{}/{}", config.session_server, session_id)
            }
        };
        
        // Always print the session URL before enabling raw mode
        eprintln!("🏖️  Session: {}", session_url_str);
        eprintln!("🏖️  Join with: beach --join {}", session_url_str);
        
        // Create the session with the generated ID
        let session_url = SessionUrl::new(&session_url_str);
        let session = Self::new(session_url, transport, passphrase);
        let mut server_session = ServerSession::new(session, cmd);
        
        // Connect WebSocket (handler will be set by caller if needed)
        if let Err(e) = server_session.connect_signaling(&config.session_server, &session_id).await {
            eprintln!("⚠️  Failed to establish WebSocket connection: {}", e);
            eprintln!("⚠️  Debug commands will not work without WebSocket connection");
        } else {
            eprintln!("✅ WebSocket connection established");
        }
        
        Ok(server_session)
    }

    /// Join an existing session
    pub async fn join(session_str: &str, transport: T, passphrase: Option<String>) -> Result<(ClientSession<T>, String, String)> {
        // Parse session URL - expecting format: server/session_id
        let parts: Vec<&str> = session_str.split('/').collect();
        let (server_addr, session_id) = if parts.len() == 2 {
            (parts[0], parts[1])
        } else {
            return Err(anyhow::anyhow!("Invalid session URL format. Expected: server/session_id"));
        };

        // Validate session with session server silently
        let session_client = client::SessionClient::new(server_addr);
        session_client.join_session(session_id, passphrase.as_deref()).await.map_err(|e| {
            anyhow::anyhow!("Failed to join session: {}", e)
        })?;
        
        let session_url = SessionUrl::parse(session_str)?;
        let session = Self::new(session_url, transport, passphrase.clone());
        let client_session = ClientSession::new(session);
        
        // Return session and connection info so caller can set handler before connecting
        Ok((client_session, server_addr.to_string(), session_id.to_string()))
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn url(&self) -> &SessionUrl {
        &self.url
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn passphrase(&self) -> &Option<String> {
        &self.passphrase
    }
}
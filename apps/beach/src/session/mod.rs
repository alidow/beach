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
use crate::subscription::SubscriptionHub;
use std::sync::OnceLock;

// Global debug log path that can be set once at startup
static DEBUG_LOG_PATH: OnceLock<Option<String>> = OnceLock::new();

/// Set the global debug log path (can only be called once)
pub fn set_debug_log_path(path: Option<String>) {
    let _ = DEBUG_LOG_PATH.set(path);
}

/// Get the global debug log path
pub fn get_debug_log_path() -> Option<&'static str> {
    DEBUG_LOG_PATH.get()
        .and_then(|p| p.as_ref())
        .map(|s| s.as_str())
}

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
    transport: Arc<tokio::sync::Mutex<T>>,
    subscription_hub: Arc<SubscriptionHub>,
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
    /// Per-client WebRTC transports
    webrtc_transports: Arc<RwLock<HashMap<String, Arc<crate::transport::webrtc::WebRTCTransport>>>>,
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
            webrtc_transports: Arc::new(RwLock::new(HashMap::new())),
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
        
        // Also start WebRTC receive loop if transport is WebRTC
        {
            let transport = self.session.transport.lock().await;
            if transport.is_webrtc() {
                drop(transport);
                if let Some(handler) = &self.handler {
                    self.start_webrtc_receive_loop(handler.clone()).await;
                }
            }
        }
    }

    /// Start WebRTC receive loop for handling incoming data channel messages
    /// Note: This is now handled per-client in the PeerJoined handler
    async fn start_webrtc_receive_loop(&self, _handler: Arc<dyn ServerMessageHandler>) {
        // Per-client routers are set up when each client joins
        // This method is kept for compatibility but does nothing
    }
    
    /// Start routing messages from WebSocket to handler
    async fn start_message_router(&self, handler: Arc<dyn ServerMessageHandler>) {
        if let Some(signaling) = &self.signaling {
            let signaling = signaling.clone();
            let signaling_sender = signaling.clone();
            let clients = self.clients.clone();
            let debug_handler = self.debug_handler.clone();
            let webrtc_channels = self.webrtc_channels.clone();
            let webrtc_transports = self.webrtc_transports.clone();
            let _transport = self.session.transport.clone();
            let subscription_hub = self.session.subscription_hub();
            
            tokio::spawn(async move {
                while let Some(msg) = signaling.recv().await {
                    // Log all incoming signaling messages for debugging
                    if let Some(debug_log_path) = get_debug_log_path() {
                        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&debug_log_path) {
                            use std::io::Write;
                            let msg_type = match &msg {
                                ServerMessage::PeerJoined { .. } => "PeerJoined",
                                ServerMessage::PeerLeft { .. } => "PeerLeft",
                                ServerMessage::Signal { .. } => "Signal",
                                ServerMessage::Debug { .. } => "Debug",
                                ServerMessage::JoinSuccess { .. } => "JoinSuccess",
                                ServerMessage::JoinError { .. } => "JoinError",
                                ServerMessage::Pong => "Pong",
                                ServerMessage::Error { .. } => "Error",
                            };
                            let _ = writeln!(file, "[{}] Server received signaling message: {}", 
                                             chrono::Utc::now().format("%H:%M:%S%.3f"), msg_type);
                        }
                    }
                    
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
                            
                            // Create per-client WebRTC transport
                            {
                                use crate::transport::webrtc::{WebRTCTransport, config::WebRTCConfigBuilder};
                                use crate::transport::TransportMode;
                                
                                // Create new WebRTC transport for this client
                                let config = match WebRTCConfigBuilder::new()
                                    .mode(TransportMode::Server)
                                    .build() {
                                    Ok(cfg) => cfg,
                                    Err(e) => {
                                        // Log error to debug file instead of stderr
                                        if let Some(debug_log_path) = get_debug_log_path() {
                                            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&debug_log_path) {
                                                use std::io::Write;
                                                let _ = writeln!(file, "[{}] Failed to build WebRTC config: {}", 
                                                                 chrono::Utc::now().format("%H:%M:%S%.3f"), e);
                                            }
                                        }
                                        continue;
                                    }
                                };
                                
                                match WebRTCTransport::new(config).await {
                                    Ok(client_transport) => {
                                        let client_transport_arc = Arc::new(client_transport);
                                        let peer_id = peer.id.clone();
                                        let webrtc_transports_clone = webrtc_transports.clone();
                                        let handler_clone = handler.clone();
                                        
                                        // Store the transport
                                        webrtc_transports_clone.write().await.insert(peer_id.clone(), client_transport_arc.clone());
                                        
                                        // Take the incoming receiver for routing
                                        if let Some(mut rx) = client_transport_arc.take_incoming().await {
                                            // Start router task for this client
                                            tokio::spawn(async move {
                                                while let Some(bytes) = rx.recv().await {
                                                    if let Ok(app_msg) = serde_json::from_slice::<AppMessage>(&bytes) {
                                                        // Log the received message type for debugging
                                                        // TODO: Add debug logging here
                                                        
                                                        handler_clone.handle_client_message(&peer_id, app_msg).await;
                                                    }
                                                }
                                            });
                                        }
                                        
                                        // Initiate WebRTC connection
                                        let transport_for_init = client_transport_arc.clone();
                                        let channel_for_init = channel_arc.clone();
                                        let peer_id_for_log = peer.id.clone();
                                        // Use already captured subscription hub for auto-subscribe after connection
                                        let _hub_for_init = subscription_hub.clone();
                                        
                                        // Log WebRTC initiation
                                        if let Some(debug_log_path) = get_debug_log_path() {
                                            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&debug_log_path) {
                                                use std::io::Write;
                                                let _ = writeln!(file, "[{}] Server initiating WebRTC as offerer for client {}", 
                                                                 chrono::Utc::now().format("%H:%M:%S%.3f"), peer_id_for_log);
                                            }
                                        }
                                        
                                        tokio::spawn(async move {
                                            let timeout_duration = std::time::Duration::from_secs(30);
                                            let result = tokio::time::timeout(
                                                timeout_duration,
                                                transport_for_init.initiate_remote_connection(
                                                    channel_for_init,
                                                    true // Server is the offerer
                                                )
                                            ).await;
                                            
                                            match result {
                                                Ok(Ok(())) => {
                                                    // Log successful connection
                                                    if let Some(debug_log_path) = get_debug_log_path() {
                                                        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&debug_log_path) {
                                                            use std::io::Write;
                                                            let _ = writeln!(file, "[{}] Server WebRTC connection established for client {}", 
                                                                             chrono::Utc::now().format("%H:%M:%S%.3f"), peer_id_for_log);
                                                        }
                                                    }

                                                    // Client should initiate its own subscription when ready
                                                    if let Some(debug_log_path) = get_debug_log_path() {
                                                        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&debug_log_path) {
                                                            use std::io::Write;
                                                            let _ = writeln!(file, "[{}] Server WebRTC ready for client {}, waiting for Subscribe message", 
                                                                             chrono::Utc::now().format("%H:%M:%S%.3f"), peer_id_for_log);
                                                        }
                                                    }
                                                }
                                                Ok(Err(e)) => {
                                                    // Log connection error
                                                    if let Some(debug_log_path) = get_debug_log_path() {
                                                        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&debug_log_path) {
                                                            use std::io::Write;
                                                            let _ = writeln!(file, "[{}] Server WebRTC connection failed for client {}: {}", 
                                                                             chrono::Utc::now().format("%H:%M:%S%.3f"), peer_id_for_log, e);
                                                        }
                                                    }
                                                    if true /* strict WebRTC is now default */ {
                                                        panic!("WebRTC connection required but failed: {}", e);
                                                    }
                                                }
                                                Err(_) => {
                                                    if true /* strict WebRTC is now default */ {
                                                        panic!("WebRTC handshake timed out");
                                                    }
                                                }
                                            }
                                        });
                                    }
                                    Err(e) => {
                                        eprintln!("Failed to create WebRTC transport for client {}: {}", peer.id, e);
                                    }
                                }
                            }
                        }
                        ServerMessage::PeerLeft { peer_id } => {
                            clients.write().await.remove(&peer_id);
                            webrtc_channels.write().await.remove(&peer_id);
                            handler.handle_client_left(&peer_id).await;
                        }
                        ServerMessage::Debug { response: _ } => {
                            // This shouldn't happen on the server side (Debug is sent TO us, not FROM signaling)
                            // But just in case...
                        }
                        ServerMessage::Signal { from_peer, signal } => {
                            // Check if this is a WebRTC signal
                            if let Ok(transport_signal) = TransportSignal::from_value(&signal) {
                                let TransportSignal::WebRTC { .. } = transport_signal;
                                // Handle WebRTC signaling
                                if let Some(channel) = webrtc_channels.read().await.get(&from_peer) {
                                    let _ = channel.handle_signal(signal.clone()).await;
                                }
                                continue;
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
        let strict_webrtc = true /* strict WebRTC is now default */;
        
        let clients = self.clients.read().await;
        
        // In strict mode, only use WebRTC for app data
        if strict_webrtc {
            // Send to each client individually via their WebRTC transport
            let transports = self.webrtc_transports.read().await;
            let bytes = serde_json::to_vec(&message)?;
            let mut errors = Vec::new();
            let mut sent_count = 0;
            
            for (client_id, client_transport) in transports.iter() {
                if clients.contains_key(client_id) && client_transport.is_connected() {
                    if let Err(e) = client_transport.send(&bytes).await {
                        errors.push(format!("{}: {}", client_id, e));
                    } else {
                        sent_count += 1;
                    }
                }
            }
            
            if sent_count == 0 && !errors.is_empty() {
                return Err(anyhow::anyhow!("WebRTC broadcast failed: {}", errors.join(", ")));
            }
            return Ok(());
        }
        
        // Non-strict mode: Try WebRTC first, fall back to WebSocket
        {
            let transports = self.webrtc_transports.read().await;
            if !transports.is_empty() {
                let bytes = serde_json::to_vec(&message)?;
                let mut any_sent = false;
                
                for (client_id, client_transport) in transports.iter() {
                    if clients.contains_key(client_id) && client_transport.is_connected() {
                        if client_transport.send(&bytes).await.is_ok() {
                            any_sent = true;
                        }
                    }
                }
                
                if any_sent {
                    return Ok(());
                }
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
        let strict_webrtc = true /* strict WebRTC is now default */;
        
        // In strict mode, only use WebRTC for app data
        if strict_webrtc {
            // Look up client's WebRTC transport
            let transports = self.webrtc_transports.read().await;
            if let Some(client_transport) = transports.get(client_id) {
                if client_transport.is_connected() {
                    let bytes = serde_json::to_vec(&message)?;
                    
                    // Determine which channel to use based on message type
                    use crate::transport::ChannelPurpose;
                    use crate::protocol::subscription::ServerMessage;
                    
                    // Check if this is a terminal data message
                    let is_terminal_data = if let AppMessage::Protocol { message: ref msg_value } = message {
                        if let Ok(server_msg) = serde_json::from_value::<ServerMessage>(msg_value.clone()) {
                            matches!(server_msg, 
                                ServerMessage::Snapshot { .. } | 
                                ServerMessage::Delta { .. } | 
                                ServerMessage::DeltaBatch { .. })
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    
                    // Use Output channel for terminal data, Control channel for everything else
                    if is_terminal_data {
                        let channel = client_transport.channel(ChannelPurpose::Output).await?;
                        return channel.send(&bytes).await
                            .map_err(|e| anyhow::anyhow!("WebRTC Output channel send to client {} failed: {}", client_id, e));
                    } else {
                        return client_transport.send(&bytes).await
                            .map_err(|e| anyhow::anyhow!("WebRTC Control channel send to client {} failed: {}", client_id, e));
                    }
                } else {
                    return Err(anyhow::anyhow!("WebRTC transport not connected for client {}", client_id));
                }
            } else {
                return Err(anyhow::anyhow!("No WebRTC transport found for client {}", client_id));
            }
        }
        
        // Non-strict mode: Try WebRTC first, fall back to WebSocket
        {
            let transports = self.webrtc_transports.read().await;
            if let Some(client_transport) = transports.get(client_id) {
                if client_transport.is_connected() {
                    let bytes = serde_json::to_vec(&message)?;
                    
                    // Determine which channel to use based on message type
                    use crate::transport::ChannelPurpose;
                    use crate::protocol::subscription::ServerMessage;
                    
                    // Check if this is a terminal data message
                    let is_terminal_data = if let AppMessage::Protocol { message: ref msg_value } = message {
                        if let Ok(server_msg) = serde_json::from_value::<ServerMessage>(msg_value.clone()) {
                            matches!(server_msg, 
                                ServerMessage::Snapshot { .. } | 
                                ServerMessage::Delta { .. } | 
                                ServerMessage::DeltaBatch { .. })
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    
                    // Use Output channel for terminal data, Control channel for everything else
                    if is_terminal_data {
                        if let Ok(channel) = client_transport.channel(ChannelPurpose::Output).await {
                            if channel.send(&bytes).await.is_ok() {
                                return Ok(());
                            }
                        }
                    } else {
                        if client_transport.send(&bytes).await.is_ok() {
                            return Ok(());
                        }
                    }
                }
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

    /// Get the per-client WebRTC transport if available
    pub async fn get_webrtc_transport(&self, client_id: &str) -> Option<Arc<crate::transport::webrtc::WebRTCTransport>> {
        let transports = self.webrtc_transports.read().await;
        transports.get(client_id).cloned()
    }
    
    pub fn subscription_hub(&self) -> Arc<SubscriptionHub> {
        self.session.subscription_hub.clone()
    }

    /// Check if any clients are connected
    pub async fn has_clients(&self) -> bool {
        let clients = self.clients.read().await;
        !clients.is_empty()
    }

    /// Check if any WebRTC connection is established
    pub async fn has_any_webrtc_connected(&self) -> bool {
        // Check if any per-client WebRTC transport is connected
        let transports = self.webrtc_transports.read().await;
        for (_, transport) in transports.iter() {
            if transport.is_connected() {
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

    /// Start WebRTC receive loop for handling incoming data channel messages
    async fn start_webrtc_receive_loop(&self, handler: Arc<dyn ClientMessageHandler>) {
        // Client-side polling with short lock windows to avoid blocking send()
        let transport = self.session.transport.clone();
        
        tokio::spawn(async move {
            loop {
                // Try to acquire the lock quickly; if busy, yield
                let lock_result = tokio::time::timeout(
                    std::time::Duration::from_millis(5),
                    transport.lock()
                ).await;

                let mut guard = match lock_result {
                    Ok(g) => g,
                    Err(_) => { 
                        tokio::time::sleep(std::time::Duration::from_millis(5)).await; 
                        continue; 
                    }
                };

                if !guard.is_webrtc() {
                    drop(guard);
                    break;
                }

                // Bound how long we hold the lock for recv()
                let recv_fut = guard.recv();
                let recv_result = tokio::time::timeout(
                    std::time::Duration::from_millis(20),
                    recv_fut
                ).await;
                drop(guard); // Release lock before processing

                match recv_result {
                    Ok(Some(bytes)) => {
                        if let Ok(app_msg) = serde_json::from_slice::<AppMessage>(&bytes) {
                            handler.handle_server_message(app_msg).await;
                        }
                    }
                    Ok(None) => {
                        tokio::time::sleep(std::time::Duration::from_millis(8)).await;
                    }
                    Err(_) => {
                        // Timed out waiting for a frame; yield and retry
                        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    }
                }
            }
        });
    }

    /// Start routing messages from WebSocket to handler
    async fn start_message_router(&self, handler: Arc<dyn ClientMessageHandler>) {
        // Start WebRTC receive loop if transport is WebRTC
        {
            let transport = self.session.transport.lock().await;
            if transport.is_webrtc() {
                drop(transport);
                self.start_webrtc_receive_loop(handler.clone()).await;
            }
        }
        
        if let Some(signaling) = &self.signaling {
            let signaling = signaling.clone();
            let signaling_sender = signaling.clone();
            let transport = self.session.transport.clone();
            
            tokio::spawn(async move {
                let mut _server_peer_id: Option<String> = None;
                let mut webrtc_channel: Option<Arc<RemoteSignalingChannel>> = None;
                
                while let Some(msg) = signaling.recv().await {
                    // Log all incoming signaling messages for debugging
                    if let Some(debug_log_path) = get_debug_log_path() {
                        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&debug_log_path) {
                            use std::io::Write;
                            let msg_type = match &msg {
                                ServerMessage::JoinSuccess { .. } => "JoinSuccess",
                                ServerMessage::PeerJoined { .. } => "PeerJoined",
                                ServerMessage::PeerLeft { .. } => "PeerLeft",
                                ServerMessage::Signal { .. } => "Signal",
                                ServerMessage::Debug { .. } => "Debug",
                                ServerMessage::JoinError { .. } => "JoinError",
                                ServerMessage::Pong => "Pong",
                                ServerMessage::Error { .. } => "Error",
                            };
                            let _ = writeln!(file, "[{}] Client received signaling message: {}", 
                                             chrono::Utc::now().format("%H:%M:%S%.3f"), msg_type);
                        }
                    }
                    
                    match msg {
                        ServerMessage::JoinSuccess { peers, .. } => {
                            // Find the server peer
                            for peer in &peers {
                                if matches!(peer.role, PeerRole::Server) {
                                    _server_peer_id = Some(peer.id.clone());
                                    
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
                            // Log Signal message reception
                            if let Some(debug_log_path) = get_debug_log_path() {
                                if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&debug_log_path) {
                                    use std::io::Write;
                                    let _ = writeln!(file, "[{}] Client processing Signal from peer: {}", 
                                                     chrono::Utc::now().format("%H:%M:%S%.3f"), from_peer);
                                }
                            }
                            
                            // Check if this is a WebRTC signal
                            if let Ok(transport_signal) = TransportSignal::from_value(&signal) {
                                let TransportSignal::WebRTC { signal: webrtc_signal } = transport_signal;
                                // Log WebRTC signal type
                                if let Some(debug_log_path) = get_debug_log_path() {
                                        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&debug_log_path) {
                                            use std::io::Write;
                                            let signal_type = match &webrtc_signal {
                                                WebRTCSignal::Offer { .. } => "Offer",
                                                WebRTCSignal::Answer { .. } => "Answer",
                                                WebRTCSignal::IceCandidate { .. } => "IceCandidate",
                                            };
                                            let _ = writeln!(file, "[{}] Client received WebRTC {} signal", 
                                                             chrono::Utc::now().format("%H:%M:%S%.3f"), signal_type);
                                        }
                                    }
                                    
                                    // If this is an Offer and we haven't initiated yet, do it now
                                    if matches!(webrtc_signal, WebRTCSignal::Offer { .. }) {
                                        let transport_guard = transport.lock().await;
                                        if transport_guard.is_webrtc() && webrtc_channel.is_some() {
                                            drop(transport_guard);
                                            let transport_clone = transport.clone();
                                            let channel_arc = webrtc_channel.as_ref().unwrap().clone();
                                            let channel_any = channel_arc.clone() as Arc<dyn std::any::Any + Send + Sync>;
                                            
                                            // First, handle the signal to store it
                                            let _ = channel_arc.handle_signal(signal.clone()).await;
                                            
                                            // Log WebRTC initialization attempt
                                            if let Some(debug_log_path) = get_debug_log_path() {
                                                if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&debug_log_path) {
                                                    use std::io::Write;
                                                    let _ = writeln!(file, "[{}] Client initiating WebRTC as answerer", 
                                                                     chrono::Utc::now().format("%H:%M:%S%.3f"));
                                                }
                                            }
                                            
                                            // Then initiate WebRTC as answerer with timeout
                                            tokio::spawn(async move {
                                                let timeout_duration = std::time::Duration::from_secs(30);
                                                let transport_guard = transport_clone.lock().await;
                                                let result = tokio::time::timeout(
                                                    timeout_duration,
                                                    transport_guard.initiate_webrtc_with_signaling(
                                                        channel_any,
                                                        false // Client is the answerer
                                                    )
                                                ).await;
                                                
                                                match result {
                                                    Ok(Ok(())) => {
                                                        // Log successful WebRTC initialization
                                                        if let Some(debug_log_path) = get_debug_log_path() {
                                                            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&debug_log_path) {
                                                                use std::io::Write;
                                                                let _ = writeln!(file, "[{}] Client WebRTC initialization successful", 
                                                                                 chrono::Utc::now().format("%H:%M:%S%.3f"));
                                                            }
                                                        }
                                                    }
                                                    Ok(Err(e)) => {
                                                        // Log WebRTC initialization error
                                                        if let Some(debug_log_path) = get_debug_log_path() {
                                                            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&debug_log_path) {
                                                                use std::io::Write;
                                                                let _ = writeln!(file, "[{}] Client WebRTC initialization failed: {}", 
                                                                                 chrono::Utc::now().format("%H:%M:%S%.3f"), e);
                                                            }
                                                        }
                                                        if true /* strict WebRTC is now default */ {
                                                            panic!("WebRTC connection required but failed: {}. Ensure both peers support WebRTC and network allows peer-to-peer connections.", e);
                                                        }
                                                    }
                                                    Err(_) => {
                                                        if true /* strict WebRTC is now default */ {
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
        let strict_webrtc = true /* strict WebRTC is now default */;
        
        // In strict mode, only use WebRTC for app data
        if strict_webrtc {
            let transport = self.session.transport.lock().await;
            if !transport.is_connected() {
                return Err(anyhow::anyhow!("WebRTC required but not connected"));
            }
            let bytes = serde_json::to_vec(&message)?;
            
            // Determine which channel to use based on message type
            use crate::protocol::subscription::ClientMessage;
            
            // Check if this is input/control data
            let is_input_control = if let AppMessage::Protocol { message: ref msg_value } = message {
                if let Ok(client_msg) = serde_json::from_value::<ClientMessage>(msg_value.clone()) {
                    matches!(client_msg,
                        ClientMessage::TerminalInput { .. } |
                        ClientMessage::Subscribe { .. } |
                        ClientMessage::Unsubscribe { .. })
                } else {
                    false
                }
            } else {
                false
            };
            
            // Use Control channel for input/control, default send for everything else
            let send_start = std::time::Instant::now();
            
            // Debug log: Before transport send
            if let Some(debug_log_path) = get_debug_log_path() {
                if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(&debug_log_path) {
                    use std::io::Write;
                    let _ = writeln!(file, "[{}] ClientSession: Starting transport.send() for {} bytes ({})",
                        chrono::Local::now().format("%H:%M:%S%.3f"), 
                        bytes.len(),
                        if is_input_control { "control" } else { "data" });
                }
            }
            
            let result = if is_input_control {
                // Control channel is used by default send
                transport.send(&bytes).await
                    .map_err(|e| anyhow::anyhow!("WebRTC Control channel send failed: {}", e))
            } else {
                transport.send(&bytes).await
                    .map_err(|e| anyhow::anyhow!("WebRTC send failed: {}", e))
            };
            
            let elapsed = send_start.elapsed();
            
            // Debug log: After transport send with timing
            if let Some(debug_log_path) = get_debug_log_path() {
                if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(&debug_log_path) {
                    use std::io::Write;
                    match &result {
                        Ok(_) => {
                            let _ = writeln!(file, "[{}] ClientSession: transport.send() completed in {}μs",
                                chrono::Local::now().format("%H:%M:%S%.3f"), elapsed.as_micros());
                            if elapsed.as_millis() > 5 {
                                let _ = writeln!(file, "[{}] ClientSession: WARNING: transport.send() took >5ms ({}ms)",
                                    chrono::Local::now().format("%H:%M:%S%.3f"), elapsed.as_millis());
                            }
                        },
                        Err(e) => {
                            let _ = writeln!(file, "[{}] ClientSession: transport.send() failed after {}μs: {:?}",
                                chrono::Local::now().format("%H:%M:%S%.3f"), elapsed.as_micros(), e);
                        }
                    }
                }
            }
            
            return result;
        }
        
        // Non-strict mode: Try WebRTC first, fall back to WebSocket
        {
            let transport = self.session.transport.lock().await;
            if transport.is_webrtc() && transport.is_connected() {
                let serialized = serde_json::to_vec(&message)?;
                if transport.send(&serialized).await.is_ok() {
                    return Ok(());
                }
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
    
    pub fn subscription_hub(&self) -> Arc<SubscriptionHub> {
        self.session.subscription_hub.clone()
    }
    
    /// Get access to the transport for channel operations
    pub fn transport(&self) -> Arc<tokio::sync::Mutex<T>> {
        self.session.transport.clone()
    }
}

impl<T: Transport + Send + 'static> Session<T> {
    pub fn new(url: SessionUrl, transport: T, passphrase: Option<String>) -> Self {
        Self { 
            url, 
            transport: Arc::new(tokio::sync::Mutex::new(transport)),
            subscription_hub: Arc::new(SubscriptionHub::new()),
            passphrase, 
            id: generate_session_id() 
        }
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

    pub fn transport(&self) -> Arc<tokio::sync::Mutex<T>> {
        self.transport.clone()
    }

    pub fn passphrase(&self) -> &Option<String> {
        &self.passphrase
    }
    
    pub fn subscription_hub(&self) -> Arc<SubscriptionHub> {
        self.subscription_hub.clone()
    }
}

pub mod client;
pub mod handlers;
pub mod signaling;
pub mod websocket;

use url::{Url, ParseError};
use crate::transport::Transport;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use anyhow::Result;
use crate::config::Config;
use self::websocket::SignalingConnection;
use self::signaling::{AppMessage, ClientMessage, ServerMessage, PeerRole};
use self::handlers::{ServerMessageHandler, ClientMessageHandler};

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
    transport: T,
    passphrase: Option<String>,
}

pub struct ServerSession<T: Transport + Send + 'static> {
    session: Session<T>,
    cmd: Vec<String>,
    clients: Arc<RwLock<HashMap<String, bool>>>, // client id, connected
    signaling: Option<Arc<SignalingConnection>>,
    handler: Option<Arc<dyn ServerMessageHandler>>,
    debug_handler: Option<Arc<crate::server::debug_handler::DebugHandler>>,
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
        }
    }

    /// Connect to session server WebSocket
    pub async fn connect_signaling(&mut self, session_server: &str, session_id: &str) -> Result<()> {
        let connection = SignalingConnection::connect(
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

    /// Set the message handler and start router if connected
    pub async fn set_handler(&mut self, handler: Arc<dyn ServerMessageHandler>) {
        self.handler = Some(handler.clone());
        // If signaling is already connected, start the message router
        if self.signaling.is_some() {
            self.start_message_router(handler).await;
        }
    }
    
    /// Set the debug handler
    pub fn set_debug_handler(&mut self, debug_handler: Arc<crate::server::debug_handler::DebugHandler>) {
        self.debug_handler = Some(debug_handler);
    }

    /// Start routing messages from WebSocket to handler
    async fn start_message_router(&self, handler: Arc<dyn ServerMessageHandler>) {
        if let Some(signaling) = &self.signaling {
            let signaling = signaling.clone();
            let signaling_sender = signaling.clone();
            let clients = self.clients.clone();
            let debug_handler = self.debug_handler.clone();
            
            tokio::spawn(async move {
                while let Some(msg) = signaling.recv().await {
                    match msg {
                        ServerMessage::PeerJoined { peer } => {
                            clients.write().await.insert(peer.id.clone(), true);
                            handler.handle_client_joined(&peer).await;
                        }
                        ServerMessage::PeerLeft { peer_id } => {
                            clients.write().await.remove(&peer_id);
                            handler.handle_client_left(&peer_id).await;
                        }
                        ServerMessage::Signal { from_peer, signal } => {
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
                                        }
                                    }
                                }
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
        if let Some(signaling) = &self.signaling {
            let clients = self.clients.read().await;
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
}

pub struct ClientSession<T: Transport + Send + 'static> {
    session: Session<T>,
    client_instance_id: String,
    signaling: Option<Arc<SignalingConnection>>,
    handler: Option<Arc<dyn ClientMessageHandler>>,
    server_peer_id: Option<String>,
}

impl<T: Transport + Send + 'static> ClientSession<T> {
    pub fn new(session: Session<T>) -> Self {
        Self {
            session,
            client_instance_id: generate_session_id(),
            signaling: None,
            handler: None,
            server_peer_id: None,
        }
    }

    /// Connect to session server WebSocket
    pub async fn connect_signaling(&mut self, session_server: &str, session_id: &str) -> Result<()> {
        let connection = SignalingConnection::connect(
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
            
            tokio::spawn(async move {
                while let Some(msg) = signaling.recv().await {
                    match msg {
                        ServerMessage::JoinSuccess { peers, .. } => {
                            // Find the server peer
                            for peer in &peers {
                                if matches!(peer.role, PeerRole::Server) {
                                    // TODO: Store server_peer_id
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
                        ServerMessage::Signal { from_peer: _, signal } => {
                            // TODO: Parse transport signal and extract app message
                            // For now, assume it's an app message
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
        if let Some(signaling) = &self.signaling {
            if let Some(server_id) = &self.server_peer_id {
                let msg = ClientMessage::Signal {
                    to_peer: server_id.clone(),
                    signal: serde_json::to_value(&message)?,
                };
                signaling.send(msg).await?;
            }
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
        Self { url, transport, passphrase, id: generate_session_id() }
    }

    /// Create a new session and register it with the session server
    pub async fn create(config: &Config, transport: T, passphrase: Option<String>, cmd: Vec<String>) -> Result<ServerSession<T>> {
        // Generate a session ID
        let session_id = generate_session_id();
        
        // Register session with session server
        let session_client = client::SessionClient::new(&config.session_server);
        let session_url_str = match session_client.register_session(&session_id, passphrase.as_deref()).await {
            Ok(url) => {
                eprintln!("🏖️  Beach Server: Session registered successfully");
                eprintln!("🏖️  Session URL: {}", url);
                eprintln!("🏖️  Others can join with: beach --join {}", url);
                url
            }
            Err(e) => {
                eprintln!("⚠️  Warning: Failed to register session with session server: {}", e);
                eprintln!("⚠️  Continuing without session server (direct connections only)");
                format!("{}/{}", config.session_server, session_id)
            }
        };
        
        // Create the session with the generated ID
        let session_url = SessionUrl::new(&session_url_str);
        let session = Self::new(session_url, transport, passphrase);
        let mut server_session = ServerSession::new(session, cmd);
        
        // Connect WebSocket for signaling
        if let Err(e) = server_session.connect_signaling(&config.session_server, &session_id).await {
            eprintln!("⚠️  Warning: Failed to connect WebSocket signaling: {}", e);
        }
        Ok(server_session)
    }

    /// Join an existing session
    pub async fn join(session_str: &str, transport: T, passphrase: Option<String>) -> Result<ClientSession<T>> {
        // Parse session URL - expecting format: server/session_id
        let parts: Vec<&str> = session_str.split('/').collect();
        let (server_addr, session_id) = if parts.len() == 2 {
            (parts[0], parts[1])
        } else {
            return Err(anyhow::anyhow!("Invalid session URL format. Expected: server/session_id"));
        };

        eprintln!("🏖️  Beach Client: Joining session {} on {}", session_id, server_addr);
        
        // Validate session with session server
        let session_client = client::SessionClient::new(server_addr);
        session_client.join_session(session_id, passphrase.as_deref()).await.map_err(|e| {
            anyhow::anyhow!("Failed to join session: {}", e)
        })?;
        
        eprintln!("🏖️  Beach Client: Session validated successfully");
        eprintln!("🏖️  Connecting to session...");
        
        let session_url = SessionUrl::parse(session_str)?;
        let session = Self::new(session_url, transport, passphrase.clone());
        let mut client_session = ClientSession::new(session);
        
        // Connect WebSocket for signaling
        if let Err(e) = client_session.connect_signaling(server_addr, session_id).await {
            eprintln!("⚠️  Warning: Failed to connect WebSocket signaling: {}", e);
        }
        
        Ok(client_session)
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
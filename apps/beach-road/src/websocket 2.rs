use anyhow::Result;
use axum::{
    extract::{ws::{Message, WebSocket}, Path, State, WebSocketUpgrade},
    response::Response,
};
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

use crate::signaling::{
    ClientMessage, ServerMessage, PeerInfo, PeerRole, TransportType, generate_peer_id
};
use crate::handlers::SharedStorage;

/// Connection state for a single WebSocket peer
#[derive(Clone)]
struct PeerConnection {
    peer_id: String,
    session_id: String,
    role: PeerRole,
    supported_transports: Vec<TransportType>,
    preferred_transport: Option<TransportType>,
    tx: mpsc::UnboundedSender<ServerMessage>,
    last_heartbeat: Arc<RwLock<std::time::Instant>>,
}

/// Global state for managing WebSocket connections
#[derive(Clone)]
pub struct SignalingState {
    /// Map of session_id -> (peer_id -> PeerConnection)
    sessions: Arc<DashMap<String, DashMap<String, PeerConnection>>>,
    /// Storage for session validation
    storage: SharedStorage,
}

impl SignalingState {
    pub fn new(storage: SharedStorage) -> Self {
        let state = Self {
            sessions: Arc::new(DashMap::new()),
            storage,
        };
        
        // Start heartbeat monitor task
        let monitor_state = state.clone();
        tokio::spawn(async move {
            monitor_state.monitor_heartbeats().await;
        });
        
        state
    }
    
    /// Monitor heartbeats and clean up stale connections
    async fn monitor_heartbeats(&self) {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60)); // Check every minute
        let timeout = std::time::Duration::from_secs(600); // 10 minute timeout
        
        loop {
            interval.tick().await;
            
            let mut stale_peers = Vec::new();
            
            // Find stale connections
            for session_entry in self.sessions.iter() {
                let session_id = session_entry.key().clone();
                let peers = session_entry.value();
                
                for peer_entry in peers.iter() {
                    let peer_id = peer_entry.key().clone();
                    let peer = peer_entry.value();
                    
                    let last_heartbeat = *peer.last_heartbeat.read().await;
                    if last_heartbeat.elapsed() > timeout {
                        stale_peers.push((session_id.clone(), peer_id.clone()));
                    }
                }
            }
            
            // Remove stale peers
            for (session_id, peer_id) in stale_peers {
                info!("Removing stale peer {} from session {} (heartbeat timeout)", peer_id, session_id);
                self.remove_peer(&session_id, &peer_id);
                
                // Notify other peers
                let _ = self.broadcast_except(&session_id, &peer_id, ServerMessage::PeerLeft {
                    peer_id: peer_id.clone(),
                }).await;
                
                // TODO: Update session status in Redis storage
            }
        }
    }

    /// Add a peer to a session
    fn add_peer(&self, session_id: String, peer: PeerConnection) {
        let peers = self.sessions.entry(session_id.clone())
            .or_insert_with(|| DashMap::new());
        peers.insert(peer.peer_id.clone(), peer);
    }

    /// Remove a peer from a session
    fn remove_peer(&self, session_id: &str, peer_id: &str) {
        if let Some(peers) = self.sessions.get(session_id) {
            peers.remove(peer_id);
            // Clean up empty sessions
            if peers.is_empty() {
                self.sessions.remove(session_id);
            }
        }
    }

    /// Get all peers in a session
    fn get_peers(&self, session_id: &str) -> Vec<PeerInfo> {
        self.sessions.get(session_id)
            .map(|peers| {
                peers.iter()
                    .map(|entry| PeerInfo {
                        id: entry.peer_id.clone(),
                        role: entry.role.clone(),
                        joined_at: chrono::Utc::now().timestamp(),
                        supported_transports: entry.supported_transports.clone(),
                        preferred_transport: entry.preferred_transport.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get common transports supported by all peers in a session
    fn get_available_transports(&self, session_id: &str) -> Vec<TransportType> {
        if let Some(peers) = self.sessions.get(session_id) {
            if peers.is_empty() {
                return vec![];
            }
            
            // Find intersection of all peer's supported transports
            let mut common_transports: Option<Vec<TransportType>> = None;
            
            for peer in peers.iter() {
                if let Some(ref mut common) = common_transports {
                    // Keep only transports that are in both lists
                    common.retain(|t| peer.supported_transports.contains(t));
                } else {
                    // First peer, start with their transports
                    common_transports = Some(peer.supported_transports.clone());
                }
            }
            
            common_transports.unwrap_or_default()
        } else {
            vec![]
        }
    }

    /// Send a message to a specific peer
    async fn send_to_peer(&self, session_id: &str, peer_id: &str, message: ServerMessage) -> Result<()> {
        if let Some(peers) = self.sessions.get(session_id) {
            if let Some(peer) = peers.get(peer_id) {
                peer.tx.send(message)
                    .map_err(|e| anyhow::anyhow!("Failed to send message: {}", e))?;
            }
        }
        Ok(())
    }

    /// Broadcast a message to all peers in a session except the sender
    async fn broadcast_except(&self, session_id: &str, sender_id: &str, message: ServerMessage) -> Result<()> {
        if let Some(peers) = self.sessions.get(session_id) {
            for peer in peers.iter() {
                if peer.peer_id != sender_id {
                    let _ = peer.tx.send(message.clone());
                }
            }
        }
        Ok(())
    }
}

/// WebSocket upgrade handler
pub async fn websocket_handler(
    ws: WebSocketUpgrade,
    Path(session_id): Path<String>,
    State(signaling): State<SignalingState>,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, session_id, signaling))
}

/// Handle a WebSocket connection
async fn handle_socket(socket: WebSocket, session_id: String, state: SignalingState) {
    let peer_id = generate_peer_id();
    let (mut sender, mut receiver) = socket.split();
    
    // Create channel for sending messages to this peer
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerMessage>();
    
    // Spawn task to forward messages from channel to WebSocket
    let peer_id_clone = peer_id.clone();
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg) {
                if sender.send(Message::Text(json)).await.is_err() {
                    break;
                }
            }
        }
        debug!("Message sender task ended for peer {}", peer_id_clone);
    });

    info!("WebSocket connected: peer={} session={}", peer_id, session_id);

    // Handle incoming messages
    while let Some(Ok(msg)) = receiver.next().await {
        if let Message::Text(text) = msg {
            match serde_json::from_str::<ClientMessage>(&text) {
                Ok(client_msg) => {
                    if let Err(e) = handle_client_message(
                        client_msg,
                        &peer_id,
                        &session_id,
                        &state,
                        &tx,
                    ).await {
                        error!("Error handling message: {}", e);
                        let _ = tx.send(ServerMessage::Error {
                            message: format!("Failed to process message: {}", e),
                        });
                    }
                }
                Err(e) => {
                    warn!("Failed to parse client message: {}", e);
                    let _ = tx.send(ServerMessage::Error {
                        message: format!("Invalid message format: {}", e),
                    });
                }
            }
        } else if let Message::Close(_) = msg {
            break;
        }
    }

    // Clean up on disconnect
    state.remove_peer(&session_id, &peer_id);
    
    // Notify other peers
    let _ = state.broadcast_except(&session_id, &peer_id, ServerMessage::PeerLeft {
        peer_id: peer_id.clone(),
    }).await;
    
    info!("WebSocket disconnected: peer={} session={}", peer_id, session_id);
}

/// Handle incoming client messages
async fn handle_client_message(
    message: ClientMessage,
    peer_id: &str,
    session_id: &str,
    state: &SignalingState,
    tx: &mpsc::UnboundedSender<ServerMessage>,
) -> Result<()> {
    match message {
        ClientMessage::Join { 
            peer_id: client_peer_id, 
            passphrase: _, 
            supported_transports,
            preferred_transport,
        } => {
            // Validate session exists and passphrase if required
            // TODO: Check with storage if session exists and passphrase matches
            
            // Determine role based on whether this is the first peer in the session
            // First peer is assumed to be the server
            let role = if state.get_peers(session_id).is_empty() {
                PeerRole::Server
            } else {
                PeerRole::Client
            };
            
            // Add peer to session
            let peer_conn = PeerConnection {
                peer_id: client_peer_id.clone(),
                session_id: session_id.to_string(),
                role: role.clone(),
                supported_transports: supported_transports.clone(),
                preferred_transport: preferred_transport.clone(),
                tx: tx.clone(),
                last_heartbeat: Arc::new(RwLock::new(std::time::Instant::now())),
            };
            state.add_peer(session_id.to_string(), peer_conn);
            
            // Get existing peers and available transports
            let peers = state.get_peers(session_id);
            let available_transports = state.get_available_transports(session_id);
            
            // Send success response
            tx.send(ServerMessage::JoinSuccess {
                session_id: session_id.to_string(),
                peer_id: client_peer_id.clone(),
                peers: peers.clone(),
                available_transports,
            })?;
            
            // Notify other peers
            state.broadcast_except(session_id, &client_peer_id, ServerMessage::PeerJoined {
                peer: PeerInfo {
                    id: client_peer_id.clone(),
                    role,
                    joined_at: chrono::Utc::now().timestamp(),
                    supported_transports,
                    preferred_transport,
                },
            }).await?;
        }
        
        ClientMessage::NegotiateTransport { to_peer, proposed_transport } => {
            state.send_to_peer(session_id, &to_peer, ServerMessage::TransportProposal {
                from_peer: peer_id.to_string(),
                proposed_transport,
            }).await?;
        }
        
        ClientMessage::AcceptTransport { to_peer, transport } => {
            state.send_to_peer(session_id, &to_peer, ServerMessage::TransportAccepted {
                from_peer: peer_id.to_string(),
                transport,
            }).await?;
        }
        
        ClientMessage::Signal { to_peer, signal } => {
            // Check if this is a debug response from the server
            if let Some(msg_type) = signal.get("type").and_then(|v| v.as_str()) {
                if msg_type == "debug_response" {
                    // This is a debug response from the server, send it as a Debug message
                    if let Some(response) = signal.get("response") {
                        // Convert the response to proper DebugResponse format
                        let debug_response = match response.get("type").and_then(|v| v.as_str()) {
                            Some("grid_view") => {
                                crate::signaling::DebugResponse::GridView {
                                    width: response.get("width").and_then(|v| v.as_u64()).unwrap_or(80) as u16,
                                    height: response.get("height").and_then(|v| v.as_u64()).unwrap_or(24) as u16,
                                    cursor_row: response.get("cursor_row").and_then(|v| v.as_u64()).unwrap_or(0) as u16,
                                    cursor_col: response.get("cursor_col").and_then(|v| v.as_u64()).unwrap_or(0) as u16,
                                    cursor_visible: response.get("cursor_visible").and_then(|v| v.as_bool()).unwrap_or(true),
                                    rows: response.get("rows")
                                        .and_then(|v| v.as_array())
                                        .map(|arr| arr.iter()
                                            .filter_map(|v| v.as_str().map(String::from))
                                            .collect())
                                        .unwrap_or_default(),
                                    ansi_rows: response.get("ansi_rows")
                                        .and_then(|v| v.as_array())
                                        .map(|arr| arr.iter()
                                            .filter_map(|v| v.as_str().map(String::from))
                                            .collect()),
                                    timestamp: chrono::Utc::now(),
                                    start_line: response.get("start_line").and_then(|v| v.as_u64()).unwrap_or(0),
                                    end_line: response.get("end_line").and_then(|v| v.as_u64()).unwrap_or(0),
                                }
                            }
                            Some("stats") => {
                                crate::signaling::DebugResponse::Stats {
                                    history_size_bytes: response.get("history_size_bytes").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
                                    total_deltas: response.get("total_deltas").and_then(|v| v.as_u64()).unwrap_or(0),
                                    total_snapshots: response.get("total_snapshots").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
                                    current_dimensions: (
                                        response.get("current_dimensions").and_then(|v| v.get(0)).and_then(|v| v.as_u64()).unwrap_or(80) as u16,
                                        response.get("current_dimensions").and_then(|v| v.get(1)).and_then(|v| v.as_u64()).unwrap_or(24) as u16,
                                    ),
                                    session_duration_secs: response.get("session_duration_secs").and_then(|v| v.as_u64()).unwrap_or(0),
                                }
                            }
                            Some("success") => {
                                crate::signaling::DebugResponse::Success {
                                    message: response.get("message").and_then(|v| v.as_str()).unwrap_or("Success").to_string(),
                                }
                            }
                            Some("error") => {
                                crate::signaling::DebugResponse::Error {
                                    message: response.get("message").and_then(|v| v.as_str()).unwrap_or("Error").to_string(),
                                }
                            }
                            _ => {
                                crate::signaling::DebugResponse::Error {
                                    message: "Unknown debug response type".to_string(),
                                }
                            }
                        };
                        
                        // Send to the requesting peer
                        state.send_to_peer(session_id, &to_peer, ServerMessage::Debug {
                            response: debug_response,
                        }).await?;
                        return Ok(());
                    }
                }
            }
            
            // Normal signal forwarding
            state.send_to_peer(session_id, &to_peer, ServerMessage::Signal {
                from_peer: peer_id.to_string(),
                signal,
            }).await?;
        }
        
        ClientMessage::Ping => {
            // Update heartbeat timestamp
            if let Some(peers) = state.sessions.get(session_id) {
                if let Some(peer) = peers.get(peer_id) {
                    *peer.last_heartbeat.write().await = std::time::Instant::now();
                }
            }
            tx.send(ServerMessage::Pong)?;
        }
        
        ClientMessage::Debug { request } => {
            // Forward debug request to the beach server
            // Find the server peer in the session
            if let Some(peers) = state.sessions.get(session_id) {
                let server_peer = peers.iter()
                    .find(|p| p.role == PeerRole::Server)
                    .map(|p| p.peer_id.clone());
                
                if let Some(server_id) = server_peer {
                    // Forward the debug request to the server via Signal
                    // Package it as a Signal message with a debug payload
                    let debug_payload = serde_json::json!({
                        "type": "debug_request",
                        "from_peer": peer_id,
                        "request": request,
                    });
                    
                    state.send_to_peer(session_id, &server_id, ServerMessage::Signal {
                        from_peer: peer_id.to_string(),
                        signal: debug_payload,
                    }).await?;
                } else {
                    // No server found, send error response
                    tx.send(ServerMessage::Error {
                        message: "No server found in session to handle debug request".to_string(),
                    })?;
                }
            } else {
                tx.send(ServerMessage::Error {
                    message: "Session not found".to_string(),
                })?;
            }
        }
    }
    
    Ok(())
}
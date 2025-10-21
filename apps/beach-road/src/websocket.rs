use anyhow::Result;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        ConnectInfo, Path, State, WebSocketUpgrade,
    },
    response::Response,
};
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

use crate::handlers::SharedStorage;
use crate::signaling::{
    generate_peer_id, ClientMessage, PeerInfo, PeerRole, ServerMessage, TransportType,
};

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
    label: Option<String>,
    remote_addr: Option<SocketAddr>,
    mcp_requested: bool,
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
            // Collect peer heartbeat locks first to avoid holding DashMap guards across await
            let mut heartbeat_checks = Vec::new();
            for session_entry in self.sessions.iter() {
                let session_id = session_entry.key().clone();
                let peers = session_entry.value();

                for peer_entry in peers.iter() {
                    let peer_id = peer_entry.key().clone();
                    let peer = peer_entry.value();
                    heartbeat_checks.push((
                        session_id.clone(),
                        peer_id.clone(),
                        peer.last_heartbeat.clone(),
                    ));
                }
            }

            // Now check heartbeats without holding DashMap guards
            for (session_id, peer_id, heartbeat_lock) in heartbeat_checks {
                let last_heartbeat = *heartbeat_lock.read().await;
                if last_heartbeat.elapsed() > timeout {
                    stale_peers.push((session_id, peer_id));
                }
            }

            // Remove stale peers
            for (session_id, peer_id) in stale_peers {
                info!(
                    "Removing stale peer {} from session {} (heartbeat timeout)",
                    peer_id, session_id
                );
                self.remove_peer(&session_id, &peer_id);

                // Notify other peers
                let _ = self
                    .broadcast_except(
                        &session_id,
                        &peer_id,
                        ServerMessage::PeerLeft {
                            peer_id: peer_id.clone(),
                        },
                    )
                    .await;

                // TODO: Update session status in Redis storage
            }
        }
    }

    /// Add a peer to a session
    fn add_peer(&self, session_id: String, peer: PeerConnection) {
        let peers = self
            .sessions
            .entry(session_id.clone())
            .or_insert_with(|| DashMap::new());
        peers.insert(peer.peer_id.clone(), peer);
    }

    /// Remove a peer from a session
    fn remove_peer(&self, session_id: &str, peer_id: &str) {
        let mut remove_session = false;

        if let Some(peers) = self.sessions.get(session_id) {
            peers.remove(peer_id);
            // Avoid holding the DashMap guard when deciding to remove the session entry.
            remove_session = peers.is_empty();
        }

        if remove_session {
            self.sessions.remove(session_id);
        }
    }

    /// Get all peers in a session
    fn get_peers(&self, session_id: &str) -> Vec<PeerInfo> {
        self.sessions
            .get(session_id)
            .map(|peers| {
                peers
                    .iter()
                    .map(|entry| PeerInfo {
                        id: entry.peer_id.clone(),
                        role: entry.role.clone(),
                        joined_at: chrono::Utc::now().timestamp(),
                        supported_transports: entry.supported_transports.clone(),
                        preferred_transport: entry.preferred_transport.clone(),
                        metadata: build_metadata(
                            entry.label.as_ref(),
                            entry.remote_addr,
                            entry.mcp_requested,
                        ),
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
    async fn send_to_peer(
        &self,
        session_id: &str,
        peer_id: &str,
        message: ServerMessage,
    ) -> Result<()> {
        if let Some(peers) = self.sessions.get(session_id) {
            if let Some(peer) = peers.get(peer_id) {
                peer.tx
                    .send(message)
                    .map_err(|e| anyhow::anyhow!("Failed to send message: {}", e))?;
            } else {
                warn!("Peer {} not found in session {}", peer_id, session_id);
                return Err(anyhow::anyhow!("Peer not found"));
            }
        } else {
            warn!("Session {} not found", session_id);
            return Err(anyhow::anyhow!("Session not found"));
        }
        Ok(())
    }

    /// Broadcast a message to all peers in a session except the sender
    async fn broadcast_except(
        &self,
        session_id: &str,
        sender_id: &str,
        message: ServerMessage,
    ) -> Result<()> {
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

fn build_metadata(
    label: Option<&String>,
    remote_addr: Option<SocketAddr>,
    mcp_requested: bool,
) -> Option<HashMap<String, String>> {
    let mut map = HashMap::new();
    if let Some(label) = label {
        let trimmed = label.trim();
        if !trimmed.is_empty() {
            map.insert("label".to_string(), trimmed.to_string());
        }
    }
    if let Some(addr) = remote_addr {
        map.insert("remote_addr".to_string(), addr.to_string());
    }
    if mcp_requested {
        map.insert("mcp".to_string(), "true".to_string());
    }
    if map.is_empty() {
        None
    } else {
        Some(map)
    }
}

/// WebSocket upgrade handler
pub async fn websocket_handler(
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
    Path(session_id): Path<String>,
    State(signaling): State<SignalingState>,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, session_id, signaling, remote_addr))
}

/// Handle a WebSocket connection
async fn handle_socket(
    socket: WebSocket,
    session_id: String,
    state: SignalingState,
    remote_addr: SocketAddr,
) {
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

    debug!(
        "WebSocket connected: peer={} session={}",
        peer_id, session_id
    );

    // Handle incoming messages
    while let Some(msg_result) = receiver.next().await {
        debug!("Received WebSocket frame from peer {}", peer_id);

        let msg = match msg_result {
            Ok(m) => m,
            Err(e) => {
                error!("WebSocket error from peer {}: {}", peer_id, e);
                break;
            }
        };

        debug!(
            "Received WebSocket message type: {:?} from peer {}",
            match &msg {
                Message::Text(_) => "Text",
                Message::Binary(_) => "Binary",
                Message::Ping(_) => "Ping",
                Message::Pong(_) => "Pong",
                Message::Close(_) => "Close",
            },
            peer_id
        );

        match msg {
            Message::Text(text) => {
                debug!("Text frame content from {}: {}", peer_id, text);
                match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(client_msg) => {
                        debug!(
                            "Successfully parsed ClientMessage from Text frame: {:?}",
                            client_msg
                        );
                        if let Err(e) = handle_client_message(
                            client_msg,
                            &peer_id,
                            &session_id,
                            &state,
                            &tx,
                            Some(remote_addr),
                        )
                        .await
                        {
                            error!("Error handling message: {}", e);
                            let _ = tx.send(ServerMessage::Error {
                                message: format!("Failed to process message: {}", e),
                            });
                        }
                    }
                    Err(e) => {
                        warn!("Failed to parse client message from Text frame: {}", e);
                        let _ = tx.send(ServerMessage::Error {
                            message: format!("Invalid message format: {}", e),
                        });
                    }
                }
            }
            Message::Binary(data) => {
                debug!("Binary frame size from {}: {} bytes", peer_id, data.len());
                // Also try to handle Binary frames containing JSON (for compatibility)
                if let Ok(text) = String::from_utf8(data.clone()) {
                    debug!("Binary frame as UTF-8 from {}: {}", peer_id, text);
                    match serde_json::from_str::<ClientMessage>(&text) {
                        Ok(client_msg) => {
                            debug!(
                                "Successfully parsed ClientMessage from Binary frame: {:?}",
                                client_msg
                            );
                            if let Err(e) = handle_client_message(
                                client_msg,
                                &peer_id,
                                &session_id,
                                &state,
                                &tx,
                                Some(remote_addr),
                            )
                            .await
                            {
                                error!("Error handling message: {}", e);
                                let _ = tx.send(ServerMessage::Error {
                                    message: format!("Failed to process message: {}", e),
                                });
                            }
                        }
                        Err(e) => {
                            debug!("Binary frame does not contain valid JSON: {}", e);
                        }
                    }
                } else {
                    debug!(
                        "Received non-UTF8 Binary frame from peer {} (first 100 bytes: {:?})",
                        peer_id,
                        &data[..std::cmp::min(100, data.len())]
                    );
                }
            }
            Message::Close(_) => {
                debug!("Received Close frame from peer {}", peer_id);
                break;
            }
            _ => {
                // Ignore Ping, Pong, and other message types
                debug!(
                    "Ignoring {:?} message from peer {}",
                    match msg {
                        Message::Ping(_) => "Ping",
                        Message::Pong(_) => "Pong",
                        _ => "Other",
                    },
                    peer_id
                );
            }
        }
    }

    // Clean up on disconnect
    state.remove_peer(&session_id, &peer_id);

    // Notify other peers
    let _ = state
        .broadcast_except(
            &session_id,
            &peer_id,
            ServerMessage::PeerLeft {
                peer_id: peer_id.clone(),
            },
        )
        .await;

    debug!(
        "WebSocket disconnected: peer={} session={}",
        peer_id, session_id
    );
}

/// Handle incoming client messages
async fn handle_client_message(
    message: ClientMessage,
    peer_id: &str,
    session_id: &str,
    state: &SignalingState,
    tx: &mpsc::UnboundedSender<ServerMessage>,
    remote_addr: Option<SocketAddr>,
) -> Result<()> {
    match message {
        ClientMessage::Join {
            peer_id: client_peer_id,
            passphrase: _,
            supported_transports,
            preferred_transport,
            label,
            mcp,
        } => {
            info!(
                "📥 RECEIVED Join message from peer {} (client_peer_id: {:?}) for session {}",
                peer_id, client_peer_id, session_id
            );

            // Validate session exists and passphrase if required
            // TODO: Check with storage if session exists and passphrase matches

            // Determine role based on whether this is the first peer in the session
            // First peer is assumed to be the server
            let role = if state.get_peers(session_id).is_empty() {
                info!("  → First peer in session, assigning role: Server");
                PeerRole::Server
            } else {
                info!("  → Session has existing peers, assigning role: Client");
                PeerRole::Client
            };

            // Add peer to session - use the WebSocket connection's peer_id
            let peer_conn = PeerConnection {
                peer_id: peer_id.to_string(), // Use WebSocket connection's peer_id
                session_id: session_id.to_string(),
                role: role.clone(),
                supported_transports: supported_transports.clone(),
                preferred_transport: preferred_transport.clone(),
                tx: tx.clone(),
                last_heartbeat: Arc::new(RwLock::new(std::time::Instant::now())),
                label: label.clone(),
                remote_addr,
                mcp_requested: mcp,
            };
            state.add_peer(session_id.to_string(), peer_conn);
            info!(
                "  → Added peer {} to session {} with role {:?}",
                peer_id, session_id, role
            );

            // Refresh session TTL on join activity
            {
                let storage = (*state.storage).clone();
                let _ = storage.update_session_ttl(session_id).await;
            }

            // Get existing peers and available transports
            let peers = state.get_peers(session_id);
            let available_transports = state.get_available_transports(session_id);
            info!(
                "  → Session now has {} peers, available transports: {:?}",
                peers.len(),
                available_transports
            );

            // Send success response - use WebSocket connection's peer_id
            let join_success = ServerMessage::JoinSuccess {
                session_id: session_id.to_string(),
                peer_id: peer_id.to_string(), // Use WebSocket connection's peer_id
                peers: peers.clone(),
                available_transports,
            };

            info!(
                "📤 SENDING JoinSuccess to peer {}: session={}, peer_id={}, peers={}, transports={:?}",
                peer_id,
                session_id,
                peer_id,
                peers.len(),
                state.get_available_transports(session_id)
            );

            tx.send(join_success)?;
            info!("  → JoinSuccess sent successfully to peer {}", peer_id);

            // Notify other peers - use WebSocket connection's peer_id
            state
                .broadcast_except(
                    session_id,
                    peer_id,
                    ServerMessage::PeerJoined {
                        peer: PeerInfo {
                            id: peer_id.to_string(), // Use WebSocket connection's peer_id
                            role,
                            joined_at: chrono::Utc::now().timestamp(),
                            supported_transports,
                            preferred_transport,
                            metadata: build_metadata(label.as_ref(), remote_addr, mcp),
                        },
                    },
                )
                .await?;
        }

        ClientMessage::NegotiateTransport {
            to_peer,
            proposed_transport,
        } => {
            state
                .send_to_peer(
                    session_id,
                    &to_peer,
                    ServerMessage::TransportProposal {
                        from_peer: peer_id.to_string(),
                        proposed_transport,
                    },
                )
                .await?;
        }

        ClientMessage::AcceptTransport { to_peer, transport } => {
            state
                .send_to_peer(
                    session_id,
                    &to_peer,
                    ServerMessage::TransportAccepted {
                        from_peer: peer_id.to_string(),
                        transport,
                    },
                )
                .await?;
        }

        ClientMessage::Signal { to_peer, signal } => {
            debug!(
                "Received Signal from {} to {}: {:?}",
                peer_id, to_peer, signal
            );
            // Don't intercept debug responses - just forward them as-is
            // The CLI expects to receive debug responses as Signal messages, not Debug messages

            // Normal signal forwarding (including debug responses)
            state
                .send_to_peer(
                    session_id,
                    &to_peer,
                    ServerMessage::Signal {
                        from_peer: peer_id.to_string(),
                        signal,
                    },
                )
                .await?;
        }

        ClientMessage::Ping => {
            // Update heartbeat timestamp
            // Clone the Arc<RwLock> to avoid holding DashMap guards across await
            let heartbeat_lock = state
                .sessions
                .get(session_id)
                .and_then(|peers| peers.get(peer_id).map(|peer| peer.last_heartbeat.clone()));

            if let Some(lock) = heartbeat_lock {
                *lock.write().await = std::time::Instant::now();
            }

            // Refresh session TTL on heartbeat
            {
                let storage = (*state.storage).clone();
                let _ = storage.update_session_ttl(session_id).await;
            }
            tx.send(ServerMessage::Pong)?;
        }

        ClientMessage::Debug { request } => {
            debug!(
                "Received debug request from peer {} for session {}",
                peer_id, session_id
            );
            // Forward debug request to the beach server
            // Find the server peer in the session
            if let Some(peers) = state.sessions.get(session_id) {
                let server_peer = peers
                    .iter()
                    .find(|p| p.role == PeerRole::Server)
                    .map(|p| p.peer_id.clone());

                debug!("Looking for server peer, found: {:?}", server_peer);
                if let Some(server_id) = server_peer {
                    // Forward the debug request to the server via Signal
                    // Package it as a Signal message with a debug payload
                    let debug_signal = serde_json::json!({
                        "transport": "custom",
                        "transport_name": "debug",
                        "signal_type": "debug_request",
                        "payload": {
                            "from_peer": peer_id,
                            "request": request,
                        },
                    });

                    debug!("Sending debug signal to server peer {}", server_id);
                    state
                        .send_to_peer(
                            session_id,
                            &server_id,
                            ServerMessage::Signal {
                                from_peer: peer_id.to_string(),
                                signal: debug_signal,
                            },
                        )
                        .await?;
                    debug!("Debug signal sent successfully");
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

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};

use crate::transport::{Transport, TransportMode, websocket::{WebSocketTransport, config::WebSocketConfigBuilder}};
use crate::protocol::signaling::{ClientMessage, ServerMessage, TransportType, PeerRole};

/// Adapter that wraps a Transport for signaling protocol use
pub struct SignalingTransport<T: Transport + Send + 'static> {
    /// Channel for sending ClientMessages
    tx: mpsc::UnboundedSender<ClientMessage>,
    /// Channel for receiving ServerMessages  
    rx: Arc<tokio::sync::RwLock<mpsc::UnboundedReceiver<ServerMessage>>>,
    /// Task handles
    bridge_task: Option<tokio::task::JoinHandle<()>>,
    heartbeat_task: Option<tokio::task::JoinHandle<()>>,
    /// Keep phantom data for type parameter
    _phantom: std::marker::PhantomData<T>,
}

impl<T: Transport + Send + 'static> SignalingTransport<T> {
    /// Create a new signaling transport from an existing transport
    pub fn new(mut transport: T) -> Self {
        let (tx_client, mut rx_client) = mpsc::unbounded_channel::<ClientMessage>();
        let (tx_server, rx_server) = mpsc::unbounded_channel::<ServerMessage>();
        
        let rx_server = Arc::new(tokio::sync::RwLock::new(rx_server));
        
        // Spawn single bridge task that owns the transport
        let bridge_task = tokio::spawn(async move {
            if std::env::var("BEACH_VERBOSE").is_ok() {
                // eprintln!("🔍 [VERBOSE] Bridge task starting");
            }
            
            loop {
                tokio::select! {
                    // Handle outgoing ClientMessages
                    Some(msg) = rx_client.recv() => {
                        if let Ok(json) = serde_json::to_string(&msg) {
                            // if std::env::var("BEACH_VERBOSE").is_ok() {
                            //     eprintln!("🔍 [VERBOSE] Bridge: Sending signaling message: {} (JSON: {})", 
                            //         match &msg {
                            //             ClientMessage::Join { .. } => "Join",
                            //             ClientMessage::Ping => "Ping",
                            //             ClientMessage::Signal { .. } => "Signal",
                            //             ClientMessage::Debug { .. } => "Debug",
                            //         },
                            //         &json
                            //     );
                            // }
                            let bytes = json.into_bytes();
                            if std::env::var("BEACH_VERBOSE").is_ok() {
                                // eprintln!("🔍 [VERBOSE] Bridge: Sending {} bytes to transport", bytes.len());
                            }
                            if let Err(e) = transport.send(&bytes).await {
                                if std::env::var("BEACH_VERBOSE").is_ok() {
                                    // eprintln!("🔍 [VERBOSE] Bridge: Failed to send: {}", e);
                                }
                                break;
                            }
                            if std::env::var("BEACH_VERBOSE").is_ok() {
                                // eprintln!("🔍 [VERBOSE] Bridge: Successfully sent to transport");
                            }
                        }
                    }
                    
                    // Handle incoming data from transport
                    data = transport.recv() => {
                        if let Some(data) = data {
                            if std::env::var("BEACH_VERBOSE").is_ok() {
                                // eprintln!("🔍 [VERBOSE] Bridge: Received {} bytes from transport", data.len());
                            }
                            
                            // Try to parse as JSON ServerMessage
                            if let Ok(text) = String::from_utf8(data) {
                                if let Ok(msg) = serde_json::from_str::<ServerMessage>(&text) {
                                    // if std::env::var("BEACH_VERBOSE").is_ok() {
                                    //     eprintln!("🔍 [VERBOSE] Bridge: Received signaling message: {}", 
                                    //         match &msg {
                                    //             ServerMessage::JoinSuccess { .. } => "JoinSuccess",
                                    //             ServerMessage::JoinError { .. } => "JoinError",
                                    //             ServerMessage::PeerJoined { .. } => "PeerJoined",
                                    //             ServerMessage::PeerLeft { .. } => "PeerLeft",
                                    //             ServerMessage::Signal { .. } => "Signal",
                                    //             ServerMessage::Error { .. } => "Error",
                                    //             ServerMessage::Pong => "Pong",
                                    //             ServerMessage::Debug { .. } => "Debug",
                                    //         }
                                    //     );
                                    // }
                                    if tx_server.send(msg).is_err() {
                                        if std::env::var("BEACH_VERBOSE").is_ok() {
                                            // eprintln!("🔍 [VERBOSE] Bridge: Failed to forward ServerMessage");
                                        }
                                        break;
                                    }
                                }
                            }
                        } else {
                            if std::env::var("BEACH_VERBOSE").is_ok() {
                                // eprintln!("🔍 [VERBOSE] Bridge: Transport recv returned None");
                            }
                            break;
                        }
                    }
                }
            }
            
            if std::env::var("BEACH_VERBOSE").is_ok() {
                // eprintln!("🔍 [VERBOSE] Bridge task ending");
            }
        });
        
        // Spawn heartbeat task
        let tx_heartbeat = tx_client.clone();
        let heartbeat_task = tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(30));
            loop {
                ticker.tick().await;
                if tx_heartbeat.send(ClientMessage::Ping).is_err() {
                    break;
                }
            }
        });
        
        Self {
            tx: tx_client,
            rx: rx_server,
            bridge_task: Some(bridge_task),
            heartbeat_task: Some(heartbeat_task),
            _phantom: std::marker::PhantomData,
        }
    }
    
    /// Connect and perform initial handshake
    pub async fn connect_with_handshake(
        transport: T,
        peer_id: String,
        passphrase: Option<String>,
        _role: PeerRole,
    ) -> Result<Self> {
        // Create the adapter
        let adapter = Self::new(transport);
        
        // Send join message
        let join_msg = ClientMessage::Join {
            peer_id,
            passphrase,
            supported_transports: vec![TransportType::WebRTC],
            preferred_transport: Some(TransportType::WebRTC),
        };
        adapter.send(join_msg).await?;
        
        // Don't consume the JoinSuccess - let the message router handle it
        // The message router needs to see JoinSuccess to set server_peer_id
        Ok(adapter)
    }
    
    /// Send a signaling message
    pub async fn send(&self, message: ClientMessage) -> Result<()> {
        self.tx.send(message)?;
        Ok(())
    }
    
    /// Receive next signaling message
    pub async fn recv(&self) -> Option<ServerMessage> {
        let mut rx = self.rx.write().await;
        rx.recv().await
    }
    
    /// Check if transport is connected
    pub async fn is_connected(&self) -> bool {
        // For now, check if bridge task is still running
        if let Some(bridge_task) = &self.bridge_task {
            !bridge_task.is_finished()
        } else {
            false
        }
    }
    
    /// Close the signaling transport
    pub async fn close(mut self) {
        // Stop tasks
        if let Some(task) = self.heartbeat_task.take() {
            task.abort();
        }
        if let Some(task) = self.bridge_task.take() {
            task.abort();
        }
    }
}

impl<T: Transport + Send + 'static> Drop for SignalingTransport<T> {
    fn drop(&mut self) {
        // Abort tasks if still running
        if let Some(task) = self.heartbeat_task.take() {
            task.abort();
        }
        if let Some(task) = self.bridge_task.take() {
            task.abort();
        }
    }
}

/// Helper to create a WebSocket-based signaling transport
pub async fn create_websocket_signaling(
    session_server: &str,
    session_id: &str,
    peer_id: String,
    passphrase: Option<String>,
    role: PeerRole,
) -> Result<SignalingTransport<WebSocketTransport>> {
    // Build WebSocket config
    let mode = match role {
        PeerRole::Server => TransportMode::Server,
        PeerRole::Client => TransportMode::Client,
    };
    
    let config = WebSocketConfigBuilder::new()
        .url(session_server.to_string())
        .path(format!("ws/{}", session_id))
        .mode(mode)
        .build()
        .map_err(|e| anyhow::anyhow!(e))?;
    
    // Connect WebSocket
    let ws_transport = WebSocketTransport::connect(config).await?;
    
    // Create signaling adapter with handshake
    SignalingTransport::connect_with_handshake(
        ws_transport,
        peer_id,
        passphrase,
        role,
    ).await
}
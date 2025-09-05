use anyhow::Result;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};

use crate::transport::{Transport, TransportMode, websocket::{WebSocketTransport, config::{WebSocketConfig, WebSocketConfigBuilder}}};
use crate::protocol::signaling::{ClientMessage, ServerMessage, TransportType, PeerRole};

/// Adapter that wraps a Transport for signaling protocol use
pub struct SignalingTransport<T: Transport + Send + 'static> {
    /// The underlying transport
    transport: Arc<tokio::sync::Mutex<T>>,
    /// Channel for sending ClientMessages
    tx: mpsc::UnboundedSender<ClientMessage>,
    /// Channel for receiving ServerMessages
    rx: Arc<tokio::sync::RwLock<mpsc::UnboundedReceiver<ServerMessage>>>,
    /// Task handles
    send_task: Option<tokio::task::JoinHandle<()>>,
    recv_task: Option<tokio::task::JoinHandle<()>>,
    heartbeat_task: Option<tokio::task::JoinHandle<()>>,
}

impl<T: Transport + Send + 'static> SignalingTransport<T> {
    /// Create a new signaling transport from an existing transport
    pub fn new(transport: T) -> Self {
        let transport = Arc::new(tokio::sync::Mutex::new(transport));
        let (tx_client, mut rx_client) = mpsc::unbounded_channel::<ClientMessage>();
        let (tx_server, rx_server) = mpsc::unbounded_channel::<ServerMessage>();
        
        let rx_server = Arc::new(tokio::sync::RwLock::new(rx_server));
        
        // Spawn send task
        let transport_send = transport.clone();
        let send_task = tokio::spawn(async move {
            while let Some(msg) = rx_client.recv().await {
                if let Ok(json) = serde_json::to_string(&msg) {
                    let bytes = json.into_bytes();
                    let transport = transport_send.lock().await;
                    if transport.send(&bytes).await.is_err() {
                        break;
                    }
                }
            }
        });
        
        // Spawn receive task
        let transport_recv = transport.clone();
        let recv_task = tokio::spawn(async move {
            loop {
                let mut transport = transport_recv.lock().await;
                if let Some(data) = transport.recv().await {
                    // Try to parse as JSON
                    if let Ok(text) = String::from_utf8(data) {
                        if let Ok(msg) = serde_json::from_str::<ServerMessage>(&text) {
                            if tx_server.send(msg).is_err() {
                                break;
                            }
                        }
                    }
                } else {
                    // No more data
                    break;
                }
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
            transport,
            tx: tx_client,
            rx: rx_server,
            send_task: Some(send_task),
            recv_task: Some(recv_task),
            heartbeat_task: Some(heartbeat_task),
        }
    }
    
    /// Connect and perform initial handshake
    pub async fn connect_with_handshake(
        mut transport: T,
        peer_id: String,
        passphrase: Option<String>,
        role: PeerRole,
    ) -> Result<Self> {
        // Create the adapter
        let mut adapter = Self::new(transport);
        
        // Send join message
        let join_msg = ClientMessage::Join {
            peer_id,
            passphrase,
            supported_transports: vec![TransportType::WebRTC],
            preferred_transport: Some(TransportType::WebRTC),
        };
        adapter.send(join_msg).await?;
        
        // Wait for response
        let timeout_duration = Duration::from_secs(5);
        match tokio::time::timeout(timeout_duration, adapter.recv()).await {
            Ok(Some(ServerMessage::JoinSuccess { .. })) => {
                // Success
                Ok(adapter)
            }
            Ok(Some(ServerMessage::JoinError { reason })) => {
                Err(anyhow::anyhow!("Failed to join session: {}", reason))
            }
            Ok(_) => {
                // Unexpected message or None, but continue
                Ok(adapter)
            }
            Err(_) => {
                // Timeout, but continue
                Ok(adapter)
            }
        }
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
        let transport = self.transport.lock().await;
        transport.is_connected()
    }
    
    /// Close the signaling transport
    pub async fn close(mut self) {
        // Stop tasks
        if let Some(task) = self.heartbeat_task.take() {
            task.abort();
        }
        if let Some(task) = self.send_task.take() {
            task.abort();
        }
        if let Some(task) = self.recv_task.take() {
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
        if let Some(task) = self.send_task.take() {
            task.abort();
        }
        if let Some(task) = self.recv_task.take() {
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
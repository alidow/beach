use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use tokio::net::TcpStream;
use tokio::time::{interval, Duration};
use std::sync::Arc;
use tokio::sync::RwLock;

use super::signaling::{ClientMessage, ServerMessage, TransportType};

/// Manages WebSocket connection to session server
pub struct SignalingConnection {
    /// Channel to send messages to WebSocket
    tx: mpsc::UnboundedSender<ClientMessage>,
    /// Channel to receive messages from WebSocket
    rx: Arc<RwLock<mpsc::UnboundedReceiver<ServerMessage>>>,
    /// WebSocket task handle
    task_handle: tokio::task::JoinHandle<()>,
    /// Heartbeat task handle
    heartbeat_handle: tokio::task::JoinHandle<()>,
}

impl SignalingConnection {
    /// Connect to session server WebSocket
    pub async fn connect(
        session_server: &str,
        session_id: &str,
        peer_id: String,
        passphrase: Option<String>,
        role: super::signaling::PeerRole,
    ) -> Result<Self> {
        // Build WebSocket URL
        let ws_url = if session_server.starts_with("ws://") || session_server.starts_with("wss://") {
            format!("{}/ws/{}", session_server, session_id)
        } else {
            // Default to ws:// for localhost, wss:// for others
            if session_server.contains("localhost") || session_server.contains("127.0.0.1") {
                format!("ws://{}/ws/{}", session_server, session_id)
            } else {
                format!("wss://{}/ws/{}", session_server, session_id)
            }
        };

        // Connect to WebSocket (use string URL, not Url type)
        let (ws_stream, _) = connect_async(&ws_url).await?;
        
        // Create channels for bidirectional communication
        let (tx_client, mut rx_client) = mpsc::unbounded_channel::<ClientMessage>();
        let (tx_server, rx_server) = mpsc::unbounded_channel::<ServerMessage>();
        
        let rx_server = Arc::new(RwLock::new(rx_server));
        let rx_server_clone = rx_server.clone();
        
        // Send initial join message
        let join_msg = ClientMessage::Join {
            peer_id: peer_id.clone(),
            passphrase,
            supported_transports: vec![TransportType::WebRTC], // TODO: Make configurable
            preferred_transport: Some(TransportType::WebRTC),
        };
        tx_client.send(join_msg)?;
        
        // Spawn WebSocket handler task
        let task_handle = tokio::spawn(async move {
            handle_websocket(ws_stream, rx_client, tx_server).await;
        });
        
        // Spawn heartbeat task
        let tx_heartbeat = tx_client.clone();
        let heartbeat_handle = tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(30)); // Send ping every 30 seconds
            loop {
                ticker.tick().await;
                if tx_heartbeat.send(ClientMessage::Ping).is_err() {
                    break;
                }
            }
        });
        
        // Wait for JoinSuccess response
        let mut rx_check = rx_server_clone.write().await;
        
        // Use a timeout to avoid hanging forever
        let timeout_duration = Duration::from_secs(5);
        match tokio::time::timeout(timeout_duration, rx_check.recv()).await {
            Ok(Some(ServerMessage::JoinSuccess { .. })) => {
                // Success
            }
            Ok(Some(ServerMessage::JoinError { reason })) => {
                return Err(anyhow::anyhow!("Failed to join session: {}", reason));
            }
            Ok(Some(_other)) => {
                // Unexpected message, but continue
            }
            Ok(None) => {
                // Channel closed unexpectedly
            }
            Err(_) => {
                // Timeout, but continue anyway
            }
        }
        
        drop(rx_check); // Release the write lock
        
        Ok(Self {
            tx: tx_client,
            rx: rx_server_clone,
            task_handle,
            heartbeat_handle,
        })
    }
    
    /// Send a message to the session server
    pub async fn send(&self, message: ClientMessage) -> Result<()> {
        self.tx.send(message)?;
        Ok(())
    }
    
    /// Receive next message from session server
    pub async fn recv(&self) -> Option<ServerMessage> {
        let mut rx = self.rx.write().await;
        rx.recv().await
    }
    
    /// Close the connection
    pub async fn close(self) {
        // Stop heartbeat
        self.heartbeat_handle.abort();
        // Close channel which will end the WebSocket task
        drop(self.tx);
        // Wait for task to finish
        let _ = self.task_handle.await;
    }
}

/// Handle WebSocket communication
async fn handle_websocket(
    ws_stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    mut rx_client: mpsc::UnboundedReceiver<ClientMessage>,
    tx_server: mpsc::UnboundedSender<ServerMessage>,
) {
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();
    
    // Spawn task to forward client messages to WebSocket
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx_client.recv().await {
            if let Ok(json) = serde_json::to_string(&msg) {
                if ws_sender.send(Message::Text(json)).await.is_err() {
                    break;
                }
            }
        }
    });
    
    // Handle incoming WebSocket messages
    while let Some(msg) = ws_receiver.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                if let Ok(server_msg) = serde_json::from_str::<ServerMessage>(&text) {
                    let _ = tx_server.send(server_msg);
                }
            }
            Ok(Message::Close(_)) | Err(_) => break,
            _ => {}
        }
    }
    
    // Stop send task
    send_task.abort();
}
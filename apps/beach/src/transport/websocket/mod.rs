use anyhow::Result;
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, Mutex as AsyncMutex, RwLock as AsyncRwLock};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use tokio::net::TcpStream;
use std::sync::Arc;

use super::{Transport, TransportMode, ChannelPurpose, TransportChannel};

pub mod config;
use config::WebSocketConfig;

/// WebSocket implementation of the Transport trait
pub struct WebSocketTransport {
    mode: TransportMode,
    tx: Arc<AsyncMutex<mpsc::UnboundedSender<Vec<u8>>>>,
    rx: Arc<AsyncRwLock<mpsc::UnboundedReceiver<Vec<u8>>>>,
    connected: Arc<AsyncRwLock<bool>>,
    ws_task: Option<tokio::task::JoinHandle<()>>,
}

impl WebSocketTransport {
    /// Create a new WebSocket transport and connect
    pub async fn connect(config: WebSocketConfig) -> Result<Self> {
        let mode = config.mode.clone();
        let url = config.build_url();
        
        // Log the URL we're connecting to
        if std::env::var("BEACH_VERBOSE").is_ok() {
            // eprintln!("🔍 [VERBOSE] Connecting to WebSocket URL: {}", url);
        }
        
        // Connect to WebSocket
        let (ws_stream, _) = connect_async(&url).await?;
        
        // Create channels for bidirectional communication
        let (tx_out, rx_out) = mpsc::unbounded_channel::<Vec<u8>>();
        let (tx_in, rx_in) = mpsc::unbounded_channel::<Vec<u8>>();
        
        let connected = Arc::new(AsyncRwLock::new(true));
        let connected_clone = connected.clone();
        
        // Spawn WebSocket handler task
        let ws_task = tokio::spawn(async move {
            handle_websocket(ws_stream, rx_out, tx_in, connected_clone).await;
        });
        
        Ok(Self {
            mode,
            tx: Arc::new(AsyncMutex::new(tx_out)),
            rx: Arc::new(AsyncRwLock::new(rx_in)),
            connected,
            ws_task: Some(ws_task),
        })
    }
    
    /// Close the WebSocket connection
    pub async fn close(&mut self) {
        *self.connected.write().await = false;
        
        // Abort the WebSocket task
        if let Some(task) = self.ws_task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

#[async_trait]
impl Transport for WebSocketTransport {
    async fn channel(&self, _purpose: ChannelPurpose) -> Result<Arc<dyn TransportChannel>> {
        // WebSocket doesn't support multiple channels yet
        // Return error to use default send/recv fallback
        Err(anyhow::anyhow!("WebSocket transport does not support multiple channels"))
    }
    
    async fn send(&self, data: &[u8]) -> Result<()> {
        if !self.is_connected() {
            return Err(anyhow::anyhow!("WebSocket not connected"));
        }
        
        // Debug logging
        if std::env::var("BEACH_VERBOSE").is_ok() {
            if let Ok(_text) = String::from_utf8(data.to_vec()) {
                // eprintln!("🔍 [VERBOSE] WebSocketTransport::send() - sending {} bytes: {}", data.len(), text);
            } else {
                // eprintln!("🔍 [VERBOSE] WebSocketTransport::send() - sending {} bytes (binary)", data.len());
            }
        }
        
        let tx = self.tx.lock().await;
        tx.send(data.to_vec())
            .map_err(|e| anyhow::anyhow!("Failed to send data: {}", e))?;
        Ok(())
    }
    
    async fn recv(&mut self) -> Option<Vec<u8>> {
        let mut rx = self.rx.write().await;
        rx.recv().await
    }
    
    fn is_connected(&self) -> bool {
        // Use try_read to avoid blocking
        self.connected.try_read()
            .map(|guard| *guard)
            .unwrap_or(false)
    }
    
    fn transport_mode(&self) -> TransportMode {
        self.mode.clone()
    }
}

/// Handle WebSocket communication
async fn handle_websocket(
    ws_stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    mut rx_out: mpsc::UnboundedReceiver<Vec<u8>>,
    tx_in: mpsc::UnboundedSender<Vec<u8>>,
    connected: Arc<AsyncRwLock<bool>>,
) {
    if std::env::var("BEACH_VERBOSE").is_ok() {
        // eprintln!("🔍 [VERBOSE] WebSocket handle_websocket started");
    }
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();
    
    // Spawn task to forward outgoing messages to WebSocket
    let send_task = tokio::spawn(async move {
        if std::env::var("BEACH_VERBOSE").is_ok() {
            // eprintln!("🔍 [VERBOSE] WebSocket send task started");
        }
        while let Some(data) = rx_out.recv().await {
            // Debug logging
            if std::env::var("BEACH_VERBOSE").is_ok() {
                if let Ok(_text) = String::from_utf8(data.clone()) {
                    // eprintln!("🔍 [VERBOSE] WebSocket send task - processing {} bytes: {}", data.len(), text);
                } else {
                    // eprintln!("🔍 [VERBOSE] WebSocket send task - processing {} bytes (binary)", data.len());
                }
            }
            
            // Check if data is valid JSON (signaling message) or binary data
            let message = if let Ok(text) = String::from_utf8(data.clone()) {
                // Try to parse as JSON to detect signaling messages
                if text.trim_start().starts_with('{') && text.trim_end().ends_with('}') {
                    // Looks like JSON, send as Text frame for beach-road compatibility
                    if std::env::var("BEACH_VERBOSE").is_ok() {
                        // eprintln!("🔍 [VERBOSE] WebSocket - sending as Text frame");
                    }
                    Message::Text(text)
                } else {
                    // Not JSON, send as binary
                    if std::env::var("BEACH_VERBOSE").is_ok() {
                        // eprintln!("🔍 [VERBOSE] WebSocket - sending as Binary frame (not JSON)");
                    }
                    Message::Binary(data)
                }
            } else {
                // Not valid UTF-8, must be binary data
                if std::env::var("BEACH_VERBOSE").is_ok() {
                    // eprintln!("🔍 [VERBOSE] WebSocket - sending as Binary frame (not UTF-8)");
                }
                Message::Binary(data)
            };
            
            if let Err(_e) = ws_sender.send(message).await {
                if std::env::var("BEACH_VERBOSE").is_ok() {
                    // eprintln!("🔍 [VERBOSE] WebSocket send failed: {}, breaking loop", e);
                }
                break;
            }
            // Flush to ensure message is sent immediately
            if let Err(_e) = ws_sender.flush().await {
                if std::env::var("BEACH_VERBOSE").is_ok() {
                    // eprintln!("🔍 [VERBOSE] WebSocket flush failed: {}, breaking loop", e);
                }
                break;
            }
            if std::env::var("BEACH_VERBOSE").is_ok() {
                // eprintln!("🔍 [VERBOSE] WebSocket message sent and flushed successfully");
            }
        }
        if std::env::var("BEACH_VERBOSE").is_ok() {
            // eprintln!("🔍 [VERBOSE] WebSocket send task ending");
        }
    });
    
    // Handle incoming WebSocket messages
    if std::env::var("BEACH_VERBOSE").is_ok() {
        // eprintln!("🔍 [VERBOSE] WebSocket receive loop starting");
    }
    while let Some(msg) = ws_receiver.next().await {
        match msg {
            Ok(Message::Binary(data)) => {
                if tx_in.send(data).is_err() {
                    break;
                }
            }
            Ok(Message::Text(text)) => {
                // Convert text to bytes
                if tx_in.send(text.into_bytes()).is_err() {
                    break;
                }
            }
            Ok(Message::Close(_)) | Err(_) => {
                break;
            }
            _ => {} // Ignore other message types (Ping, Pong, etc.)
        }
    }
    
    // Mark as disconnected
    *connected.write().await = false;
    
    // Stop send task
    send_task.abort();
    let _ = send_task.await;
}

impl Drop for WebSocketTransport {
    fn drop(&mut self) {
        // Abort WebSocket task if still running
        if let Some(task) = self.ws_task.take() {
            task.abort();
        }
    }
}
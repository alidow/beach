use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use async_trait::async_trait;
use anyhow::Result;

use crate::subscription::manager::SubscriptionManager;
use crate::transport::Transport;
use crate::protocol::{ClientMessage, ServerMessage, Dimensions, ViewMode};
use crate::server::terminal_state::TerminalStateTracker;

use super::ServerMessageHandler;
use crate::protocol::signaling::{AppMessage, PeerInfo};

/// Bridges WebSocket signaling with the subscription system
pub struct SubscriptionHandler<T: Transport + Send + 'static> {
    subscription_manager: Arc<SubscriptionManager<T>>,
    
    /// Maps peer_id -> mpsc channel for sending messages to that client
    client_channels: Arc<RwLock<HashMap<String, mpsc::Sender<ServerMessage>>>>,
    
    /// Maps peer_id -> mpsc receiver for receiving from that client
    client_receivers: Arc<RwLock<HashMap<String, mpsc::Receiver<ClientMessage>>>>,
}

impl<T: Transport + Send + Sync + 'static> SubscriptionHandler<T> {
    pub fn new(
        transport: T,
        terminal_tracker: Arc<std::sync::Mutex<TerminalStateTracker>>,
    ) -> Self {
        Self {
            subscription_manager: Arc::new(SubscriptionManager::new(transport, terminal_tracker)),
            client_channels: Arc::new(RwLock::new(HashMap::new())),
            client_receivers: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    pub fn subscription_manager(&self) -> Arc<SubscriptionManager<T>> {
        self.subscription_manager.clone()
    }
    
    /// Called when we need to send a message to a specific client via WebSocket
    async fn send_to_client(&self, peer_id: &str, message: ServerMessage) -> Result<()> {
        let channels = self.client_channels.read().await;
        if let Some(tx) = channels.get(peer_id) {
            tx.send(message).await?;
        }
        Ok(())
    }
    
    /// Process a ClientMessage from a peer
    async fn handle_protocol_message(&self, peer_id: &str, message: ClientMessage) -> Result<()> {
        match message {
            ClientMessage::Subscribe { 
                subscription_id, 
                dimensions, 
                mode, 
                position, 
                compression: _,
            } => {
                // Create a channel for this subscription to send messages
                let (tx, mut rx) = mpsc::channel::<ServerMessage>(100);
                
                // Add the subscription
                self.subscription_manager.add_subscription(
                    subscription_id.clone(),
                    peer_id.to_string(),
                    dimensions,
                    mode,
                    position,
                    tx,
                    true, // TODO: Determine if client should be controlling
                ).await?;
                
                // Forward messages from subscription to WebSocket
                let peer_id = peer_id.to_string();
                let channels = self.client_channels.clone();
                tokio::spawn(async move {
                    while let Some(msg) = rx.recv().await {
                        if let Some(tx) = channels.read().await.get(&peer_id) {
                            let _ = tx.send(msg).await;
                        }
                    }
                });
            }
            
            // All other messages can be handled directly by the manager
            _ => {
                self.subscription_manager.handle_client_message(
                    peer_id.to_string(),
                    message,
                ).await?;
            }
        }
        
        Ok(())
    }
}

#[async_trait]
impl<T: Transport + Send + Sync + 'static> ServerMessageHandler for SubscriptionHandler<T> {
    async fn handle_client_joined(&self, peer: &PeerInfo) {
        eprintln!("📺 Client joined: {} (role: {:?})", peer.id, peer.role);
        
        // Create channels for this client
        let (tx, _rx) = mpsc::channel::<ServerMessage>(100);
        let (_client_tx, client_rx) = mpsc::channel::<ClientMessage>(100);
        
        self.client_channels.write().await.insert(peer.id.clone(), tx);
        self.client_receivers.write().await.insert(peer.id.clone(), client_rx);
        
        // TODO: Start task to receive messages from this client's channel
    }
    
    async fn handle_client_left(&self, peer_id: &str) {
        eprintln!("👋 Client left: {}", peer_id);
        
        // Clean up client
        self.client_channels.write().await.remove(peer_id);
        self.client_receivers.write().await.remove(peer_id);
        
        // Remove all subscriptions for this client
        let _ = self.subscription_manager.remove_client(&peer_id.to_string()).await;
    }
    
    async fn handle_client_message(&self, from_peer: &str, message: AppMessage) {
        match message {
            AppMessage::Protocol { message } => {
                // Handle subscription protocol messages
                match serde_json::from_value::<ClientMessage>(message) {
                    Ok(client_msg) => {
                        eprintln!("📥 Protocol message from {}: {:?}", from_peer, client_msg);
                        
                        if let Err(e) = self.handle_protocol_message(from_peer, client_msg).await {
                            eprintln!("❌ Error handling message from {}: {}", from_peer, e);
                        }
                    }
                    Err(e) => {
                        eprintln!("⚠️  Failed to parse protocol message from {}: {}", from_peer, e);
                    }
                }
            }
            AppMessage::TerminalInput { data } => {
                // Legacy terminal input - convert to protocol message
                let client_msg = ClientMessage::TerminalInput { 
                    data,
                    echo_local: None,
                };
                let _ = self.subscription_manager.handle_client_message(
                    from_peer.to_string(),
                    client_msg,
                ).await;
            }
            AppMessage::TerminalResize { cols, rows } => {
                // TODO: Handle resize - update all subscriptions for this client
                eprintln!("📐 Terminal resize from {}: {}x{}", from_peer, cols, rows);
            }
            _ => {
                eprintln!("⚠️  Unhandled message type from {}: {:?}", from_peer, message);
            }
        }
    }
}
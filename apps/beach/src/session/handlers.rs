use async_trait::async_trait;
use super::signaling::{AppMessage, PeerInfo};

/// Trait for handling messages in a beach server
#[async_trait]
pub trait ServerMessageHandler: Send + Sync {
    /// Handle message from a client
    async fn handle_client_message(&self, from_peer: &str, message: AppMessage);
    
    /// Handle client joined event
    async fn handle_client_joined(&self, peer: &PeerInfo);
    
    /// Handle client left event
    async fn handle_client_left(&self, peer_id: &str);
}

/// Trait for handling messages in a beach client
#[async_trait]
pub trait ClientMessageHandler: Send + Sync {
    /// Handle message from the server
    async fn handle_server_message(&self, message: AppMessage);
    
    /// Handle peer joined event (another client joined)
    async fn handle_peer_joined(&self, peer: &PeerInfo);
    
    /// Handle peer left event (another client left)
    async fn handle_peer_left(&self, peer_id: &str);
}
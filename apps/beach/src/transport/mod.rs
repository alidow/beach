use anyhow::Result;
use async_trait::async_trait;

pub mod mock;
pub mod websocket;
pub mod webrtc;

#[derive(Debug, Clone)]
pub enum TransportMode {
    Server,
    Client,
}

/// Transport trait for abstracting network communication
#[async_trait]
pub trait Transport: Send + Sync {
    /// Send data to the remote peer
    async fn send(&self, data: &[u8]) -> Result<()>;
    
    /// Receive data from the remote peer
    async fn recv(&mut self) -> Option<Vec<u8>>;
    
    /// Check if the transport is connected
    fn is_connected(&self) -> bool;
    
    /// Get the transport mode (Server or Client)
    fn transport_mode(&self) -> TransportMode;
}
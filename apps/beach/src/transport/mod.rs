use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

pub mod channel;
pub mod mock;
pub mod websocket;
pub mod webrtc;

pub use channel::{ChannelPurpose, ChannelReliability, TransportChannel, ChannelOptions};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportMode {
    Server,
    Client,
}

/// Transport trait for abstracting network communication
#[async_trait]
pub trait Transport: Send + Sync {
    /// Get or create a channel by purpose
    async fn channel(&self, purpose: ChannelPurpose) -> Result<Arc<dyn TransportChannel>>;
    
    /// List all open channels
    fn channels(&self) -> Vec<ChannelPurpose> {
        vec![]
    }
    
    /// Check if transport supports multiple channels
    fn supports_multi_channel(&self) -> bool {
        false
    }
    
    /// Send data to the remote peer (uses control channel by default)
    async fn send(&self, data: &[u8]) -> Result<()> {
        // Default implementation: route through control channel for backward compatibility
        let channel = self.channel(ChannelPurpose::Control).await?;
        channel.send(data).await
    }
    
    /// Receive data from the remote peer (uses control channel by default)
    async fn recv(&mut self) -> Option<Vec<u8>>;
    
    /// Check if the transport is connected
    fn is_connected(&self) -> bool;
    
    /// Get the transport mode (Server or Client)
    fn transport_mode(&self) -> TransportMode;
    
    /// Initiate WebRTC connection with remote signaling (only implemented by WebRTCTransport)
    /// Default implementation does nothing - allows non-WebRTC transports to ignore this
    async fn initiate_webrtc_with_signaling(
        &self, 
        _signaling: Arc<dyn std::any::Any + Send + Sync>,
        _is_offerer: bool
    ) -> Result<()> {
        Ok(()) // Default no-op for non-WebRTC transports
    }
    
    /// Check if this transport is WebRTC-based
    fn is_webrtc(&self) -> bool {
        false // Default to false for non-WebRTC transports
    }
}
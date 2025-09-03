//! Transport Module - Network transport implementations for client-server communication
//!
//! This module provides various transport mechanisms for Beach terminal communication,
//! with support for both WebRTC (peer-to-peer with dual channels) and WebSocket
//! (client-server) protocols. The transport layer abstracts the underlying network
//! details, providing a uniform interface for sending and receiving terminal data.
//!
//! # Submodules
//!
//! - `webrtc`: WebRTC transport with dual-channel support (reliable/unreliable)
//! - `websocket`: WebSocket transport for browser compatibility

pub mod webrtc;

use anyhow::Result;
use async_trait::async_trait;
use crate::session::SessionUrl;

#[derive(Debug, Clone)]
pub enum TransportMode {
    Server,
    Client(SessionUrl),
}

/// Generic transport trait for sending/receiving data
/// Implementations can be WebRTC, WebSocket, TCP, etc.
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    /// Send data over the transport
    async fn send(&self, data: &[u8]) -> Result<()>;
    
    /// Receive data from the transport (blocking)
    async fn recv(&mut self) -> Option<Vec<u8>>;
    
    /// Check if transport is connected
    fn is_connected(&self) -> bool;

    fn transport_mode(&self) -> TransportMode;
}


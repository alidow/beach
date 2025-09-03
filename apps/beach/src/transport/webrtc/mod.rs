
#[derive(Clone)]
pub struct WebRTCTransport {
    // connection: Option<WebRTCConnection>,
}

impl WebRTCTransport {
    pub fn new() -> Self {
        Self {}
    }
}

use anyhow::Result;
use async_trait::async_trait;
use crate::transport::{Transport, TransportMode};

#[async_trait]
impl Transport for WebRTCTransport {
    async fn send(&self, _data: &[u8]) -> Result<()> {
        Ok(())
    }

    async fn recv(&mut self) -> Option<Vec<u8>> {
        None
    }

    fn is_connected(&self) -> bool {
        false
    }

    fn transport_mode(&self) -> TransportMode {
        TransportMode::Server
    }
}


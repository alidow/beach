use super::{ChannelPurpose, ChannelReliability, Transport, TransportChannel, TransportMode};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// Mock channel for testing
pub struct MockChannel {
    purpose: ChannelPurpose,
    tx: Arc<Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>>,
    rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<Vec<u8>>>>>,
}

#[async_trait]
impl TransportChannel for MockChannel {
    fn label(&self) -> &str {
        "mock-channel"
    }

    fn reliability(&self) -> ChannelReliability {
        ChannelReliability::Reliable
    }

    fn purpose(&self) -> ChannelPurpose {
        self.purpose
    }

    async fn send(&self, data: &[u8]) -> Result<()> {
        let tx_guard = self.tx.lock().unwrap();
        if let Some(tx) = tx_guard.as_ref() {
            tx.send(data.to_vec())
                .map_err(|e| anyhow::anyhow!("Send error: {}", e))?;
        }
        Ok(())
    }

    async fn recv(&mut self) -> Option<Vec<u8>> {
        // Clone the receiver to avoid holding the lock across await
        let rx_clone = {
            let mut rx_guard = self.rx.lock().unwrap();
            rx_guard.take()
        };

        if let Some(mut rx) = rx_clone {
            let result = rx.recv().await;
            // Put the receiver back
            let mut rx_guard = self.rx.lock().unwrap();
            *rx_guard = Some(rx);
            result
        } else {
            None
        }
    }

    fn is_open(&self) -> bool {
        true
    }
}

pub struct MockTransport {
    mode: TransportMode,
    tx: Arc<Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>>,
    rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<Vec<u8>>>>>,
    channels: Arc<Mutex<std::collections::HashMap<ChannelPurpose, Arc<MockChannel>>>>,
}

impl MockTransport {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            mode: TransportMode::Server,
            tx: Arc::new(Mutex::new(Some(tx))),
            rx: Arc::new(Mutex::new(Some(rx))),
            channels: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    pub fn new_with_mode(mode: TransportMode) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            mode,
            tx: Arc::new(Mutex::new(Some(tx))),
            rx: Arc::new(Mutex::new(Some(rx))),
            channels: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }
}

#[async_trait]
impl Transport for MockTransport {
    async fn channel(&self, purpose: ChannelPurpose) -> Result<Arc<dyn TransportChannel>> {
        let mut channels = self.channels.lock().unwrap();
        if let Some(channel) = channels.get(&purpose) {
            return Ok(channel.clone() as Arc<dyn TransportChannel>);
        }

        // Create new channel
        let (tx, rx) = mpsc::unbounded_channel();
        let channel = Arc::new(MockChannel {
            purpose,
            tx: Arc::new(Mutex::new(Some(tx))),
            rx: Arc::new(Mutex::new(Some(rx))),
        });
        channels.insert(purpose, channel.clone());
        Ok(channel as Arc<dyn TransportChannel>)
    }

    async fn send(&self, data: &[u8]) -> Result<()> {
        let tx_guard = self.tx.lock().unwrap();
        if let Some(tx) = tx_guard.as_ref() {
            tx.send(data.to_vec())
                .map_err(|e| anyhow::anyhow!("Send error: {}", e))?;
        }
        Ok(())
    }

    async fn recv(&mut self) -> Option<Vec<u8>> {
        // For testing, we don't actually receive anything
        // This prevents the test from hanging
        None
    }

    fn is_connected(&self) -> bool {
        true
    }

    fn transport_mode(&self) -> TransportMode {
        self.mode.clone()
    }
}

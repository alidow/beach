use anyhow::Result;
use async_trait::async_trait;
use super::{Transport, TransportMode};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

pub struct MockTransport {
    mode: TransportMode,
    tx: Arc<Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>>,
    rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<Vec<u8>>>>>,
}

impl MockTransport {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            mode: TransportMode::Server,
            tx: Arc::new(Mutex::new(Some(tx))),
            rx: Arc::new(Mutex::new(Some(rx))),
        }
    }
    
    pub fn new_with_mode(mode: TransportMode) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            mode,
            tx: Arc::new(Mutex::new(Some(tx))),
            rx: Arc::new(Mutex::new(Some(rx))),
        }
    }
}

#[async_trait]
impl Transport for MockTransport {
    async fn send(&self, data: &[u8]) -> Result<()> {
        let tx_guard = self.tx.lock().unwrap();
        if let Some(tx) = tx_guard.as_ref() {
            tx.send(data.to_vec()).map_err(|e| anyhow::anyhow!("Send error: {}", e))?;
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
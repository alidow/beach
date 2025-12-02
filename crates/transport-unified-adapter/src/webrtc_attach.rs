use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use serde::Deserialize;
use thiserror::Error;
use tokio::sync::broadcast;
use transport_bus::BusError;

use crate::{ExtensionFrame, ExtensionTransport, UnifiedBus};

/// Minimal trait to abstract the host/client transport we already have in `apps/beach`.
#[async_trait]
pub trait Transport: Send + Sync {
    fn subscribe_extensions(&self, namespace: &str) -> broadcast::Receiver<ExtensionFrame>;
    fn send_extension(&self, namespace: &str, topic: &str, payload: Bytes) -> Result<(), BusError>;
    fn id(&self) -> String;
}

#[derive(Debug, Error)]
pub enum AttachError {
    #[error("attach failed: {0}")]
    Attach(String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct AttachResponse {
    pub peer_session_id: String,
}

/// Wrap an existing transport as an ExtensionTransport for UnifiedBus.
struct TransportBridge {
    inner: Arc<dyn Transport>,
}

impl TransportBridge {
    fn new(inner: Arc<dyn Transport>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl ExtensionTransport for TransportBridge {
    fn subscribe_extensions(&self, namespace: &str) -> broadcast::Receiver<ExtensionFrame> {
        self.inner.subscribe_extensions(namespace)
    }

    fn send_extension(&self, namespace: &str, topic: &str, payload: Bytes) -> Result<(), BusError> {
        self.inner.send_extension(namespace, topic, payload)
    }

    fn id(&self) -> String {
        self.inner.id()
    }
}

/// Build a unified bus over an existing transport (host or client side).
pub fn build_unified_bus(transport: Arc<dyn Transport>, namespace: &str) -> Arc<UnifiedBus> {
    Arc::new(UnifiedBus::new(
        Arc::new(TransportBridge::new(transport)),
        namespace.to_string(),
    ))
}

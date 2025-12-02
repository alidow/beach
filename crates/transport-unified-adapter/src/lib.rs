use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use bytes::Bytes;
use parking_lot::RwLock;
use thiserror::Error;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use transport_bus::{Bus, BusError, BusMessage, BusResult, LocalBus};

/// Errors constructing or using the unified bus adapter.
#[derive(Debug, Error)]
pub enum UnifiedBusError {
    #[error("transport not ready: {0}")]
    NotReady(String),
    #[error("transport closed")]
    Closed,
}

/// Raw extension frame flowing over the unified transport.
#[derive(Debug, Clone)]
pub struct ExtensionFrame {
    pub namespace: String,
    pub topic: String,
    pub payload: Bytes,
}

/// Minimal trait a transport must satisfy to back the unified bus.
#[async_trait]
pub trait ExtensionTransport: Send + Sync {
    fn subscribe_extensions(&self, namespace: &str) -> broadcast::Receiver<ExtensionFrame>;
    fn send_extension(&self, namespace: &str, topic: &str, payload: Bytes) -> Result<(), BusError>;
    fn id(&self) -> String;
}

/// A pub/sub bus backed by a transport that implements [`ExtensionTransport`].
pub struct UnifiedBus {
    transport: Arc<dyn ExtensionTransport>,
    namespace: String,
    topics: Arc<RwLock<HashMap<String, broadcast::Sender<BusMessage>>>>,
    pump: OnceLock<JoinHandle<()>>,
}

impl UnifiedBus {
    pub fn new(transport: Arc<dyn ExtensionTransport>, namespace: impl Into<String>) -> Self {
        Self {
            transport,
            namespace: namespace.into(),
            topics: Arc::new(RwLock::new(HashMap::new())),
            pump: OnceLock::new(),
        }
    }

    fn sender_for(&self, topic: &str) -> broadcast::Sender<BusMessage> {
        let mut guard = self.topics.write();
        guard
            .entry(topic.to_string())
            .or_insert_with(|| broadcast::channel(128).0)
            .clone()
    }

    fn ensure_pump(&self) {
        if self.pump.get().is_some() {
            return;
        }
        let mut rx = self.transport.subscribe_extensions(&self.namespace);
        let topics: Arc<RwLock<HashMap<String, broadcast::Sender<BusMessage>>>> =
            Arc::clone(&self.topics);
        let handle = tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(frame) => {
                        let topic = frame.topic.clone();
                        let payload = frame.payload.clone();
                        let sender = {
                            let mut guard = topics.write();
                            guard
                                .entry(topic.clone())
                                .or_insert_with(|| broadcast::channel(128).0)
                                .clone()
                        };
                        let _ = sender.send(BusMessage { topic, payload });
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        // swallow lag warnings in adapter; downstream can add metrics
                        let _ = skipped;
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        let _ = self.pump.set(handle);
    }
}

impl Bus for UnifiedBus {
    fn subscribe(&self, topic: &str) -> broadcast::Receiver<BusMessage> {
        self.ensure_pump();
        self.sender_for(topic).subscribe()
    }

    fn publish(&self, topic: &str, payload: Bytes) -> BusResult<()> {
        self.transport
            .send_extension(&self.namespace, topic, payload.clone())?;
        // Also fan out locally so in-process subscribers see the message.
        let sender = self.sender_for(topic);
        let _ = sender.send(BusMessage {
            topic: topic.to_string(),
            payload,
        });
        Ok(())
    }
}

/// Trait to build a topic-based bus over the unified transport for a host attachment.
#[async_trait]
pub trait UnifiedBusAdapter: Send + Sync {
    async fn build_bus(&self, host_session_id: &str) -> Result<Arc<dyn Bus>, UnifiedBusError>;
}

/// Simple in-memory adapter for tests/dev before the RTC shim is wired.
#[derive(Default, Clone)]
pub struct IpcUnifiedAdapter {
    inner: Arc<LocalBus>,
}

impl IpcUnifiedAdapter {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(LocalBus::new()),
        }
    }

    pub fn split(&self) -> (Arc<dyn Bus>, Arc<dyn Bus>) {
        (self.inner.clone(), self.inner.clone())
    }
}

#[async_trait]
impl UnifiedBusAdapter for IpcUnifiedAdapter {
    async fn build_bus(&self, _host_session_id: &str) -> Result<Arc<dyn Bus>, UnifiedBusError> {
        Ok(self.inner.clone())
    }
}

/// Lightweight wrapper around a user-supplied Bus to avoid generic proliferation.
#[derive(Clone)]
pub struct DynBus(pub Arc<dyn Bus>);

impl Bus for DynBus {
    fn subscribe(&self, topic: &str) -> broadcast::Receiver<BusMessage> {
        self.0.subscribe(topic)
    }

    fn publish(&self, topic: &str, payload: Bytes) -> BusResult<()> {
        self.0.publish(topic, payload)
    }
}

#[cfg(feature = "webrtc-adapter")]
pub mod webrtc_attach;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ipc_adapter_round_trip() {
        let adapter = IpcUnifiedAdapter::new();
        let (mgr_bus, client_bus) = adapter.split();
        let mut rx = mgr_bus.subscribe("beach.manager.action");
        client_bus
            .publish("beach.manager.action", Bytes::from_static(b"ping"))
            .expect("publish ok");
        let msg = rx.recv().await.expect("recv ok");
        assert_eq!(msg.payload, Bytes::from_static(b"ping"));
    }
}

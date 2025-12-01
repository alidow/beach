use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use bytes::Bytes;
use parking_lot::RwLock;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::debug;

use transport_bus::{Bus, BusError, BusMessage, BusResult};

use crate::protocol::ExtensionFrame;
use crate::transport::{ExtensionDirection, ExtensionLane, Transport, extensions};

const DEFAULT_NAMESPACE: &str = "manager";

/// A pub/sub bus backed by the unified transport extension channel.
pub struct UnifiedBus {
    transport: Arc<dyn Transport>,
    namespace: String,
    outbound_direction: ExtensionDirection,
    topics: Arc<RwLock<HashMap<String, broadcast::Sender<BusMessage>>>>,
    pump: OnceLock<JoinHandle<()>>,
}

impl UnifiedBus {
    pub fn new(
        transport: Arc<dyn Transport>,
        outbound_direction: ExtensionDirection,
        namespace: impl Into<String>,
    ) -> Self {
        Self {
            transport,
            namespace: namespace.into(),
            outbound_direction,
            topics: Arc::new(parking_lot::RwLock::new(HashMap::new())),
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
                        let topic = frame.kind.clone();
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
                        debug!(
                            target = "transport.bus",
                            skipped, "bus receiver lagged on namespace"
                        );
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
        let frame = ExtensionFrame {
            namespace: self.namespace.clone(),
            kind: topic.to_string(),
            payload,
        };
        self.transport
            .send_extension(
                self.outbound_direction,
                frame.clone(),
                ExtensionLane::ControlOrdered,
            )
            .map_err(|err| BusError::Transport(err.to_string()))?;
        // publish locally for any in-process subscribers
        extensions::publish(self.transport.id(), frame);
        Ok(())
    }
}

#[cfg(test)]
impl UnifiedBus {
    pub fn inject_frame(&self, frame: ExtensionFrame) {
        self.ensure_pump();
        let topic = frame.kind.clone();
        let sender = self.sender_for(&topic);
        let _ = sender.send(BusMessage {
            topic,
            payload: frame.payload.clone(),
        });
    }
}

/// Helper to create a default manager bus from a transport.
pub fn manager_bus_from_host(transport: Arc<dyn Transport>) -> UnifiedBus {
    UnifiedBus::new(
        transport,
        ExtensionDirection::HostToClient,
        DEFAULT_NAMESPACE.to_string(),
    )
}

pub fn manager_bus_from_client(transport: Arc<dyn Transport>) -> UnifiedBus {
    UnifiedBus::new(
        transport,
        ExtensionDirection::ClientToHost,
        DEFAULT_NAMESPACE.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::ipc;

    #[test]
    fn unified_bus_round_trip_ipc() {
        let pair = ipc::build_pair().expect("ipc pair");
        let host_bus = manager_bus_from_host(Arc::from(pair.server));
        let client_bus = manager_bus_from_client(Arc::from(pair.client));

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async {
            let mut rx = host_bus.subscribe("controller/input");
            client_bus
                .publish("controller/input", Bytes::from_static(b"ping"))
                .expect("publish ok");

            // Simulate the remote decode path by injecting the frame on the host side.
            host_bus.inject_frame(ExtensionFrame {
                namespace: DEFAULT_NAMESPACE.to_string(),
                kind: "controller/input".into(),
                payload: Bytes::from_static(b"ping"),
            });

            let msg = rx.recv().await.expect("receive ok");
            assert_eq!(msg.topic, "controller/input");
            assert_eq!(msg.payload, Bytes::from_static(b"ping"));
        });
    }
}

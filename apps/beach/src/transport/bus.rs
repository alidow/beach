use std::sync::Arc;

use bytes::Bytes;
use transport_bus::BusError;
use transport_unified_adapter::{ExtensionTransport, UnifiedBus};

use crate::protocol::ExtensionFrame;
use crate::transport::{ExtensionDirection, ExtensionLane, Transport, extensions};

const DEFAULT_NAMESPACE: &str = "manager";

/// Bridge the existing transport into the shared unified bus adapter.
struct TransportBridge {
    transport: Arc<dyn Transport>,
    direction: ExtensionDirection,
}

impl TransportBridge {
    fn new(transport: Arc<dyn Transport>, direction: ExtensionDirection) -> Self {
        Self {
            transport,
            direction,
        }
    }
}

#[async_trait::async_trait]
impl ExtensionTransport for TransportBridge {
    fn subscribe_extensions(
        &self,
        namespace: &str,
    ) -> tokio::sync::broadcast::Receiver<transport_unified_adapter::ExtensionFrame> {
        let mut rx = self.transport.subscribe_extensions(namespace);
        let (tx, rx_bridge) = tokio::sync::broadcast::channel(128);
        tokio::spawn(async move {
            while let Ok(frame) = rx.recv().await {
                let _ = tx.send(transport_unified_adapter::ExtensionFrame {
                    namespace: frame.namespace.clone(),
                    topic: frame.kind.clone(),
                    payload: frame.payload.clone(),
                });
            }
        });
        rx_bridge
    }

    fn send_extension(&self, namespace: &str, topic: &str, payload: Bytes) -> Result<(), BusError> {
        let frame = ExtensionFrame {
            namespace: namespace.to_string(),
            kind: topic.to_string(),
            payload: payload.clone(),
        };
        self.transport
            .send_extension(self.direction, frame.clone(), ExtensionLane::ControlOrdered)
            .map_err(|e| BusError::Transport(e.to_string()))?;
        // Publish locally for in-process subscribers.
        extensions::publish(self.transport.id(), frame);
        Ok(())
    }

    fn id(&self) -> String {
        format!("{:?}", self.transport.id())
    }
}

/// Helper to create a default manager bus from a transport.
pub fn manager_bus_from_host(transport: Arc<dyn Transport>) -> UnifiedBus {
    let bridge = Arc::new(TransportBridge::new(
        transport,
        ExtensionDirection::HostToClient,
    ));
    UnifiedBus::new(bridge, DEFAULT_NAMESPACE)
}

pub fn manager_bus_from_client(transport: Arc<dyn Transport>) -> UnifiedBus {
    let bridge = Arc::new(TransportBridge::new(
        transport,
        ExtensionDirection::ClientToHost,
    ));
    UnifiedBus::new(bridge, DEFAULT_NAMESPACE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::ipc;
    use beach_buggy::{
        ActionCommand, ControllerBusPublisher, ControllerBusSubscriber, TOPIC_CONTROLLER_ACK,
        TOPIC_CONTROLLER_INPUT,
    };
    use std::sync::Mutex;
    use std::time::Duration;

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

    #[test]
    fn controller_input_ack_round_trip_ipc() {
        let pair = ipc::build_pair().expect("ipc pair");
        let host_bus = Arc::new(manager_bus_from_host(Arc::from(pair.server)));
        let client_bus = Arc::new(manager_bus_from_client(Arc::from(pair.client)));

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async {
            let writes: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
            let host_publisher = ControllerBusPublisher::new(host_bus.clone());
            let mut host_ack_rx = host_bus.subscribe(TOPIC_CONTROLLER_ACK);
            ControllerBusSubscriber::new(host_bus.clone()).spawn_terminal_worker(
                {
                    let writes = writes.clone();
                    move |bytes: &[u8]| {
                        writes
                            .lock()
                            .unwrap()
                            .push(String::from_utf8_lossy(bytes).to_string());
                        Ok(())
                    }
                },
                host_publisher.clone(),
            );

            let mut client_ack_rx = client_bus.subscribe(TOPIC_CONTROLLER_ACK);
            let action = ActionCommand {
                id: "bus-1".into(),
                action_type: "terminal_write".into(),
                payload: serde_json::json!({"bytes": "hi over bus"}),
                expires_at: None,
            };
            let payload = serde_json::to_vec(&serde_json::json!({
                "type": "action",
                "payload": action,
            }))
            .expect("serialize action");
            host_bus.inject_frame(ExtensionFrame {
                namespace: DEFAULT_NAMESPACE.to_string(),
                kind: TOPIC_CONTROLLER_INPUT.to_string(),
                payload: Bytes::from(payload),
            });

            let ack_msg = tokio::time::timeout(Duration::from_secs(2), host_ack_rx.recv())
                .await
                .expect("host ack timeout")
                .expect("host ack");
            client_bus.inject_frame(ExtensionFrame {
                namespace: DEFAULT_NAMESPACE.to_string(),
                kind: TOPIC_CONTROLLER_ACK.to_string(),
                payload: ack_msg.payload.clone(),
            });
            let ack_msg = tokio::time::timeout(Duration::from_secs(2), client_ack_rx.recv())
                .await
                .expect("client ack timeout")
                .expect("client ack");
            let envelope: serde_json::Value =
                serde_json::from_slice(&ack_msg.payload).expect("decode ack envelope");
            assert_eq!(envelope["type"], "ack");
            assert_eq!(envelope["payload"]["id"], "bus-1");

            let seen = writes.lock().unwrap();
            assert_eq!(seen.as_slice(), ["hi over bus"]);
        });
    }
}

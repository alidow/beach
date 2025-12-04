use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tracing::warn;
use transport_bus::Bus;

use crate::queue::{ActionAck, ActionCommand, ControllerQueue, StateDiff};
use tokio::task::JoinHandle;

#[derive(Deserialize, Serialize)]
struct Envelope<T> {
    #[serde(rename = "type")]
    kind: String,
    payload: T,
}

pub async fn ingest_message(topic: &str, payload: &[u8], queue: Arc<dyn ControllerQueue>) {
    match topic {
        t if t.ends_with(".action") => {
            if let Ok(env) = serde_json::from_slice::<Envelope<ActionCommand>>(payload) {
                queue.enqueue_action(env.payload).await;
            } else {
                warn!(topic, "failed to parse action envelope");
            }
        }
        t if t.ends_with(".ack") => {
            if let Ok(env) = serde_json::from_slice::<Envelope<ActionAck>>(payload) {
                queue.enqueue_ack(env.payload).await;
            } else {
                warn!(topic, "failed to parse ack envelope");
            }
        }
        t if t.ends_with(".state") => {
            if let Ok(env) = serde_json::from_slice::<Envelope<StateDiff>>(payload) {
                queue.enqueue_state(env.payload).await;
            } else {
                warn!(topic, "failed to parse state envelope");
            }
        }
        _ => {
            warn!(topic, "ignoring unknown bus topic");
        }
    }
}

const MANAGER_TOPICS: &[&str] = &[
    "beach.manager.action",
    "beach.manager.ack",
    "beach.manager.state",
    "beach.manager.health",
];

/// Subscribe to manager topics on the given bus and enqueue into the queue.
pub fn start_bus_ingest(bus: Arc<dyn Bus>, queue: Arc<dyn ControllerQueue>) -> Vec<JoinHandle<()>> {
    MANAGER_TOPICS
        .iter()
        .map(|topic| {
            let mut sub = bus.subscribe(topic);
            let queue = queue.clone();
            tokio::spawn(async move {
                loop {
                    match sub.recv().await {
                        Ok(msg) => {
                            ingest_message(topic, &msg.payload, queue.clone()).await;
                        }
                        Err(err) => {
                            warn!(topic, error = %err, "bus ingest recv error");
                            break;
                        }
                    }
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::InMemoryPersistence;
    use crate::pipeline;
    use crate::queue::{ControllerQueue, InMemoryQueue};
    use transport_bus::LocalBus;

    #[tokio::test]
    async fn enqueues_action_from_bus() {
        let queue = Arc::new(tokio::sync::Mutex::new(InMemoryQueue::new()));
        let payload = serde_json::to_vec(&Envelope {
            kind: "action".into(),
            payload: ActionCommand {
                id: "a1".into(),
                action_type: "write".into(),
                payload: serde_json::json!({"bytes": "hi"}),
            },
        })
        .unwrap();

        ingest_message("beach.manager.action", &payload, queue.clone()).await;
        let actions = queue.drain_actions(10).await;
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].id, "a1");
    }

    #[tokio::test]
    async fn bus_ingest_listens_on_topics() {
        let queue = Arc::new(tokio::sync::Mutex::new(InMemoryQueue::new()));
        let bus = Arc::new(LocalBus::new());
        let handles = start_bus_ingest(bus.clone() as Arc<dyn Bus>, queue.clone());

        let payload = serde_json::to_vec(&Envelope {
            kind: "action".into(),
            payload: ActionCommand {
                id: "a-bus".into(),
                action_type: "write".into(),
                payload: serde_json::json!({"bytes": "bus"}),
            },
        })
        .unwrap();
        let _ = bus.publish("beach.manager.action", payload.into());

        let mut found = false;
        for _ in 0..5 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let actions = queue.drain_actions(10).await;
            if actions.iter().any(|a| a.id == "a-bus") {
                found = true;
                break;
            }
        }
        assert!(found, "expected action from bus ingest");

        for handle in handles {
            handle.abort();
        }
    }

    #[tokio::test]
    async fn bus_to_persistence_roundtrip() {
        let queue = Arc::new(tokio::sync::Mutex::new(InMemoryQueue::new()));
        let persistence = InMemoryPersistence::new();
        let bus = Arc::new(LocalBus::new());
        let handles = start_bus_ingest(bus.clone() as Arc<dyn Bus>, queue.clone());

        // Publish action to bus
        let action_payload = serde_json::to_vec(&Envelope {
            kind: "action".into(),
            payload: ActionCommand {
                id: "act-1".into(),
                action_type: "write".into(),
                payload: serde_json::json!({"host_session_id": "h1", "controller_session_id": "c1"}),
            },
        })
        .unwrap();
        let _ = bus.publish("beach.manager.action", action_payload.into());
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        pipeline::drain_once(queue.clone(), persistence.clone(), 16, None).await;
        let actions = persistence.actions().await;
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].id, "act-1");

        // Publish ack and ensure it persists as lease
        let ack_payload = serde_json::to_vec(&Envelope {
            kind: "ack".into(),
            payload: ActionAck {
                id: "ack-1".into(),
                status: "ok".into(),
                applied_at: std::time::SystemTime::now(),
            },
        })
        .unwrap();
        let _ = bus.publish("beach.manager.ack", ack_payload.into());
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        pipeline::drain_once(queue.clone(), persistence.clone(), 16, None).await;
        let leases = persistence.leases().await;
        assert!(leases.iter().any(|l| l.lease_id == "ack-1"));

        for handle in handles {
            handle.abort();
        }
    }

    #[tokio::test]
    #[ignore = "awaiting unified transport shim; replace LocalBus with unified bus/IPC transport"]
    async fn unified_bus_ipc_placeholder() {
        let queue = Arc::new(tokio::sync::Mutex::new(InMemoryQueue::new()));
        let persistence = InMemoryPersistence::new();
        let bus = Arc::new(LocalBus::new());
        let _handles = start_bus_ingest(bus.clone(), queue.clone());
        let payload = serde_json::to_vec(&Envelope {
            kind: "action".into(),
            payload: ActionCommand {
                id: "ipc-act".into(),
                action_type: "write".into(),
                payload: serde_json::json!({"host_session_id": "h-ipc"}),
            },
        })
        .unwrap();
        let _ = bus.publish("beach.manager.action", payload.into());
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        pipeline::drain_once(queue.clone(), persistence.clone(), 8, None).await;
    }
}

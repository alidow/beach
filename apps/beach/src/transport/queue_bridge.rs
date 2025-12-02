use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use beach_buggy::{
    ActionAck, ActionCommand, StateDiff,
    publisher::{
        TOPIC_CONTROLLER_ACK, TOPIC_CONTROLLER_HEALTH, TOPIC_CONTROLLER_INPUT,
        TOPIC_CONTROLLER_STATE,
    },
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::task::JoinHandle;
use tracing::{debug, warn};
use transport_bus::Bus;

use crate::transport::queue::ControllerQueue;

#[derive(Default)]
pub struct QueueBridgeMetrics {
    pub actions_enqueued: AtomicU64,
    pub acks_enqueued: AtomicU64,
    pub states_enqueued: AtomicU64,
    pub actions_failed: AtomicU64,
    pub acks_failed: AtomicU64,
    pub states_failed: AtomicU64,
}

fn parse_enveloped<T: DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let payload = value.get("payload")?;
    serde_json::from_value(payload.clone()).ok()
}

async fn enqueue_or_warn(
    queue: Arc<dyn ControllerQueue>,
    metrics_ok: &AtomicU64,
    metrics_err: &AtomicU64,
    action: ActionEnvelope,
) {
    match action {
        ActionEnvelope::Action(cmd) => match queue.enqueue_action(cmd).await {
            Ok(()) => {
                metrics_ok.fetch_add(1, Ordering::Relaxed);
            }
            Err(err) => {
                metrics_err.fetch_add(1, Ordering::Relaxed);
                warn!(target = "queue.bridge", error = %err, "failed to enqueue action");
            }
        },
        ActionEnvelope::Ack(ack) => match queue.enqueue_ack(ack).await {
            Ok(()) => {
                metrics_ok.fetch_add(1, Ordering::Relaxed);
            }
            Err(err) => {
                metrics_err.fetch_add(1, Ordering::Relaxed);
                warn!(target = "queue.bridge", error = %err, "failed to enqueue ack");
            }
        },
        ActionEnvelope::State(diff) => match queue.enqueue_state(diff).await {
            Ok(()) => {
                metrics_ok.fetch_add(1, Ordering::Relaxed);
            }
            Err(err) => {
                metrics_err.fetch_add(1, Ordering::Relaxed);
                warn!(target = "queue.bridge", error = %err, "failed to enqueue state");
            }
        },
    }
}

enum ActionEnvelope {
    Action(ActionCommand),
    Ack(ActionAck),
    State(StateDiff),
}

/// Spawn listeners that bridge bus topics into the controller queue.
pub fn spawn_bus_queue_bridge<B>(
    bus: Arc<B>,
    queue: Arc<dyn ControllerQueue>,
    metrics: Arc<QueueBridgeMetrics>,
) -> Vec<JoinHandle<()>>
where
    B: Bus + 'static,
{
    let mut handles = Vec::new();

    // Actions
    {
        let mut rx = bus.subscribe(TOPIC_CONTROLLER_INPUT);
        let queue = Arc::clone(&queue);
        let metrics = Arc::clone(&metrics);
        handles.push(tokio::spawn(async move {
            while let Ok(msg) = rx.recv().await {
                if let Ok(text) = std::str::from_utf8(&msg.payload) {
                    if let Ok(action) = beach_buggy::fast_path::parse_action_payload(text) {
                        enqueue_or_warn(
                            Arc::clone(&queue),
                            &metrics.actions_enqueued,
                            &metrics.actions_failed,
                            ActionEnvelope::Action(action),
                        )
                        .await;
                    } else {
                        metrics.actions_failed.fetch_add(1, Ordering::Relaxed);
                        warn!(target = "queue.bridge", "dropping unparsable action");
                    }
                }
            }
        }));
    }

    // Acks
    {
        let mut rx = bus.subscribe(TOPIC_CONTROLLER_ACK);
        let queue = Arc::clone(&queue);
        let metrics = Arc::clone(&metrics);
        handles.push(tokio::spawn(async move {
            while let Ok(msg) = rx.recv().await {
                if let Some(ack) = parse_enveloped::<ActionAck>(&msg.payload) {
                    enqueue_or_warn(
                        Arc::clone(&queue),
                        &metrics.acks_enqueued,
                        &metrics.acks_failed,
                        ActionEnvelope::Ack(ack),
                    )
                    .await;
                } else {
                    metrics.acks_failed.fetch_add(1, Ordering::Relaxed);
                    warn!(target = "queue.bridge", "dropping unparsable ack");
                }
            }
        }));
    }

    // State
    {
        let mut rx = bus.subscribe(TOPIC_CONTROLLER_STATE);
        let queue = Arc::clone(&queue);
        let metrics = Arc::clone(&metrics);
        handles.push(tokio::spawn(async move {
            while let Ok(msg) = rx.recv().await {
                if let Some(state) = parse_enveloped::<StateDiff>(&msg.payload) {
                    enqueue_or_warn(
                        Arc::clone(&queue),
                        &metrics.states_enqueued,
                        &metrics.states_failed,
                        ActionEnvelope::State(state),
                    )
                    .await;
                } else {
                    metrics.states_failed.fetch_add(1, Ordering::Relaxed);
                    warn!(target = "queue.bridge", "dropping unparsable state diff");
                }
            }
        }));
    }

    // Health is observed but not enqueued; just trace so we see heartbeats.
    {
        let mut rx = bus.subscribe(TOPIC_CONTROLLER_HEALTH);
        handles.push(tokio::spawn(async move {
            while let Ok(msg) = rx.recv().await {
                debug!(
                    target = "queue.bridge",
                    len = msg.payload.len(),
                    "health heartbeat observed on bus"
                );
            }
        }));
    }

    handles
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::queue::InMemoryQueue;
    use beach_buggy::{AckStatus, publisher::ControllerBusPublisher};
    use serde_json::json;
    use std::time::SystemTime;
    use tokio::time::timeout;
    use transport_bus::LocalBus;

    #[tokio::test]
    async fn enqueues_bus_messages_into_queue() {
        let bus = Arc::new(LocalBus::new());
        let queue = Arc::new(tokio::sync::Mutex::new(InMemoryQueue::new(4, 4, 4)));
        let queue_bridge: Arc<dyn ControllerQueue> = queue.clone();
        let metrics = Arc::new(QueueBridgeMetrics::default());
        let handles = spawn_bus_queue_bridge(bus.clone(), queue_bridge, metrics.clone());

        // Publish action
        let action = ActionCommand {
            id: "a1".into(),
            action_type: "terminal_write".into(),
            payload: serde_json::json!({"bytes": "hi"}),
            expires_at: None,
        };
        let payload =
            serde_json::to_vec(&serde_json::json!({"type": "action", "payload": action})).unwrap();
        bus.publish(TOPIC_CONTROLLER_INPUT, payload.into()).unwrap();

        // Publish ack
        let ack = ActionAck {
            id: "a1".into(),
            status: AckStatus::Ok,
            applied_at: std::time::SystemTime::now(),
            latency_ms: None,
            error_code: None,
            error_message: None,
        };
        let payload =
            serde_json::to_vec(&serde_json::json!({"type": "ack", "payload": ack})).unwrap();
        bus.publish(TOPIC_CONTROLLER_ACK, payload.into()).unwrap();

        // Publish state
        let state = StateDiff {
            sequence: 1,
            emitted_at: std::time::SystemTime::now(),
            payload: serde_json::json!({"seq": 1}),
        };
        let payload =
            serde_json::to_vec(&serde_json::json!({"type": "state", "payload": state})).unwrap();
        bus.publish(TOPIC_CONTROLLER_STATE, payload.into()).unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let stats = queue.lock().await.stats();
        assert_eq!(stats.actions, 1);
        assert_eq!(stats.acks, 1);
        assert_eq!(stats.states, 1);

        // Cleanup tasks
        for handle in handles {
            handle.abort();
        }
    }

    #[tokio::test]
    async fn replay_actions_from_queue_and_emit_acks() {
        let bus = Arc::new(LocalBus::new());
        let queue = Arc::new(tokio::sync::Mutex::new(InMemoryQueue::new(8, 8, 8)));
        let queue_bridge: Arc<dyn ControllerQueue> = queue.clone();
        let metrics = Arc::new(QueueBridgeMetrics::default());
        let handles = spawn_bus_queue_bridge(bus.clone(), queue_bridge, metrics);

        // Publish action to the bus; bridge should enqueue it.
        let action = ActionCommand {
            id: "qa1".into(),
            action_type: "terminal_write".into(),
            payload: serde_json::json!({"bytes": "ping"}),
            expires_at: None,
        };
        let payload =
            serde_json::to_vec(&json!({"type": "action", "payload": action})).expect("serialize");
        bus.publish(TOPIC_CONTROLLER_INPUT, payload.into())
            .expect("publish action");
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;

        // Drain from queue and emit ack/state back onto the bus.
        let actions = {
            let mut guard = queue.lock().await;
            guard.drain_actions(10)
        };
        let mut ack_rx = bus.subscribe(TOPIC_CONTROLLER_ACK);
        let mut state_rx = bus.subscribe(TOPIC_CONTROLLER_STATE);
        let publisher = ControllerBusPublisher::new(bus.clone());
        for act in &actions {
            let ack = ActionAck {
                id: act.id.clone(),
                status: AckStatus::Ok,
                applied_at: SystemTime::now(),
                latency_ms: None,
                error_code: None,
                error_message: None,
            };
            publisher.publish_ack(&ack).expect("publish ack");
            let state = StateDiff {
                sequence: 1,
                emitted_at: SystemTime::now(),
                payload: serde_json::json!({"id": act.id.clone()}),
            };
            publisher.publish_state(&state).expect("publish state");
        }

        let ack_msg = timeout(std::time::Duration::from_secs(1), ack_rx.recv())
            .await
            .expect("ack timeout")
            .expect("ack msg");
        let state_msg = timeout(std::time::Duration::from_secs(1), state_rx.recv())
            .await
            .expect("state timeout")
            .expect("state msg");

        let ack_env: serde_json::Value = serde_json::from_slice(&ack_msg.payload).expect("ack env");
        let state_env: serde_json::Value =
            serde_json::from_slice(&state_msg.payload).expect("state env");
        assert_eq!(ack_env["payload"]["id"], "qa1");
        assert_eq!(state_env["payload"]["sequence"], 1);

        for handle in handles {
            handle.abort();
        }
    }
}

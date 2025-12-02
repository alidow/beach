use std::collections::VecDeque;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::config::QueueBackend;
use std::sync::Arc;

/// Minimal action/ack/state payloads. In the rewrite these will be sourced from bus messages.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActionCommand {
    pub id: String,
    pub action_type: String,
    pub payload: serde_json::Value,
}

pub type QueueHandle = Arc<dyn ControllerQueue>;

pub fn build_queue(backend: &QueueBackend, redis_url: Option<&str>) -> QueueHandle {
    match backend {
        QueueBackend::InMemory => Arc::new(tokio::sync::Mutex::new(InMemoryQueue::new())),
        QueueBackend::Redis => {
            if let Some(url) = redis_url {
                match crate::queue_redis::RedisQueue::connect(url) {
                    Ok(q) => return Arc::new(q),
                    Err(err) => {
                        warn!(error = %err, "failed to init redis queue; falling back to memory")
                    }
                }
            } else {
                warn!("BEACH_QUEUE_BACKEND=redis but REDIS_URL missing; falling back to in-memory");
            }
            Arc::new(tokio::sync::Mutex::new(InMemoryQueue::new()))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActionAck {
    pub id: String,
    pub status: String,
    pub applied_at: std::time::SystemTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StateDiff {
    pub sequence: u64,
    pub emitted_at: std::time::SystemTime,
    pub payload: serde_json::Value,
}

#[async_trait]
pub trait ControllerQueue: Send + Sync {
    async fn enqueue_action(&self, action: ActionCommand);
    async fn enqueue_ack(&self, ack: ActionAck);
    async fn enqueue_state(&self, state: StateDiff);
    async fn drain_actions(&self, max: usize) -> Vec<ActionCommand>;
    async fn drain_acks(&self, max: usize) -> Vec<ActionAck>;
    async fn drain_states(&self, max: usize) -> Vec<StateDiff>;
}

/// Simple in-memory queue for early wiring/testing.
#[derive(Default)]
pub struct InMemoryQueue {
    actions: VecDeque<ActionCommand>,
    acks: VecDeque<ActionAck>,
    states: VecDeque<StateDiff>,
}

impl InMemoryQueue {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ControllerQueue for tokio::sync::Mutex<InMemoryQueue> {
    async fn enqueue_action(&self, action: ActionCommand) {
        self.lock().await.actions.push_back(action);
    }

    async fn enqueue_ack(&self, ack: ActionAck) {
        self.lock().await.acks.push_back(ack);
    }

    async fn enqueue_state(&self, state: StateDiff) {
        self.lock().await.states.push_back(state);
    }

    async fn drain_actions(&self, max: usize) -> Vec<ActionCommand> {
        let mut guard = self.lock().await;
        drain(&mut guard.actions, max)
    }

    async fn drain_acks(&self, max: usize) -> Vec<ActionAck> {
        let mut guard = self.lock().await;
        drain(&mut guard.acks, max)
    }

    async fn drain_states(&self, max: usize) -> Vec<StateDiff> {
        let mut guard = self.lock().await;
        drain(&mut guard.states, max)
    }
}

fn drain<T>(queue: &mut VecDeque<T>, max: usize) -> Vec<T> {
    let mut out = Vec::with_capacity(max.min(queue.len()));
    for _ in 0..max {
        if let Some(item) = queue.pop_front() {
            out.push(item);
        } else {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn drains_insertion_order() {
        let queue = tokio::sync::Mutex::new(InMemoryQueue::new());
        queue
            .enqueue_action(ActionCommand {
                id: "a1".into(),
                action_type: "write".into(),
                payload: serde_json::json!({"bytes": "hi"}),
            })
            .await;
        queue
            .enqueue_action(ActionCommand {
                id: "a2".into(),
                action_type: "write".into(),
                payload: serde_json::json!({"bytes": "bye"}),
            })
            .await;
        let drained = queue.drain_actions(10).await;
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].id, "a1");
        assert_eq!(drained[1].id, "a2");
    }
}

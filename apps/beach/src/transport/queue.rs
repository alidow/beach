use std::collections::VecDeque;

use async_trait::async_trait;
use beach_buggy::{ActionAck, ActionCommand, StateDiff};
use thiserror::Error;
use tokio::sync::Mutex;

/// Simple in-memory queue for controller action/ack/state messages with
/// drop-count backpressure tracking. Intended as the first step before
/// swapping in a Redis-backed adapter.
#[derive(Default)]
pub struct InMemoryQueue {
    actions: VecDeque<ActionCommand>,
    acks: VecDeque<ActionAck>,
    states: VecDeque<StateDiff>,
    cap_actions: usize,
    cap_acks: usize,
    cap_states: usize,
    drops_actions: usize,
    drops_acks: usize,
    drops_states: usize,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct QueueStats {
    pub actions: usize,
    pub acks: usize,
    pub states: usize,
    pub drops_actions: usize,
    pub drops_acks: usize,
    pub drops_states: usize,
}

impl InMemoryQueue {
    pub fn new(cap_actions: usize, cap_acks: usize, cap_states: usize) -> Self {
        Self {
            actions: VecDeque::with_capacity(cap_actions),
            acks: VecDeque::with_capacity(cap_acks),
            states: VecDeque::with_capacity(cap_states),
            cap_actions,
            cap_acks,
            cap_states,
            drops_actions: 0,
            drops_acks: 0,
            drops_states: 0,
        }
    }

    pub fn stats(&self) -> QueueStats {
        QueueStats {
            actions: self.actions.len(),
            acks: self.acks.len(),
            states: self.states.len(),
            drops_actions: self.drops_actions,
            drops_acks: self.drops_acks,
            drops_states: self.drops_states,
        }
    }

    pub fn enqueue_action(&mut self, action: ActionCommand) {
        if self.actions.len() >= self.cap_actions {
            self.drops_actions += 1;
        } else {
            self.actions.push_back(action);
        }
    }

    pub fn enqueue_ack(&mut self, ack: ActionAck) {
        if self.acks.len() >= self.cap_acks {
            self.drops_acks += 1;
        } else {
            self.acks.push_back(ack);
        }
    }

    pub fn enqueue_state(&mut self, state: StateDiff) {
        if self.states.len() >= self.cap_states {
            self.drops_states += 1;
        } else {
            self.states.push_back(state);
        }
    }

    pub fn drain_actions(&mut self, max: usize) -> Vec<ActionCommand> {
        drain_queue(&mut self.actions, max)
    }

    pub fn drain_acks(&mut self, max: usize) -> Vec<ActionAck> {
        drain_queue(&mut self.acks, max)
    }

    pub fn drain_states(&mut self, max: usize) -> Vec<StateDiff> {
        drain_queue(&mut self.states, max)
    }
}

fn drain_queue<T>(queue: &mut VecDeque<T>, max: usize) -> Vec<T> {
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

#[derive(Debug, Error)]
pub enum QueueError {
    #[error("queue backend error: {0}")]
    Backend(String),
}

#[async_trait]
pub trait ControllerQueue: Send + Sync {
    async fn enqueue_action(&self, action: ActionCommand) -> Result<(), QueueError>;
    async fn enqueue_ack(&self, ack: ActionAck) -> Result<(), QueueError>;
    async fn enqueue_state(&self, state: StateDiff) -> Result<(), QueueError>;
}

#[async_trait]
impl ControllerQueue for Mutex<InMemoryQueue> {
    async fn enqueue_action(&self, action: ActionCommand) -> Result<(), QueueError> {
        let mut queue = self.lock().await;
        queue.enqueue_action(action);
        Ok(())
    }

    async fn enqueue_ack(&self, ack: ActionAck) -> Result<(), QueueError> {
        let mut queue = self.lock().await;
        queue.enqueue_ack(ack);
        Ok(())
    }

    async fn enqueue_state(&self, state: StateDiff) -> Result<(), QueueError> {
        let mut queue = self.lock().await;
        queue.enqueue_state(state);
        Ok(())
    }
}

#[async_trait]
pub trait ControllerQueueConsumer: Send + Sync {
    async fn drain_actions(&self, max: usize) -> Result<Vec<ActionCommand>, QueueError>;
    async fn drain_acks(&self, max: usize) -> Result<Vec<ActionAck>, QueueError>;
    async fn drain_states(&self, max: usize) -> Result<Vec<StateDiff>, QueueError>;
}

#[async_trait]
impl ControllerQueueConsumer for Mutex<InMemoryQueue> {
    async fn drain_actions(&self, max: usize) -> Result<Vec<ActionCommand>, QueueError> {
        let mut queue = self.lock().await;
        Ok(queue.drain_actions(max))
    }

    async fn drain_acks(&self, max: usize) -> Result<Vec<ActionAck>, QueueError> {
        let mut queue = self.lock().await;
        Ok(queue.drain_acks(max))
    }

    async fn drain_states(&self, max: usize) -> Result<Vec<StateDiff>, QueueError> {
        let mut queue = self.lock().await;
        Ok(queue.drain_states(max))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beach_buggy::{AckStatus, StateDiff};
    use std::time::SystemTime;

    fn dummy_action(id: &str) -> ActionCommand {
        ActionCommand {
            id: id.to_string(),
            action_type: "terminal_write".into(),
            payload: serde_json::json!({ "bytes": format!("data-{id}") }),
            expires_at: None,
        }
    }

    fn dummy_ack(id: &str) -> ActionAck {
        ActionAck {
            id: id.to_string(),
            status: AckStatus::Ok,
            applied_at: SystemTime::now(),
            latency_ms: None,
            error_code: None,
            error_message: None,
        }
    }

    fn dummy_state(seq: u64) -> StateDiff {
        StateDiff {
            sequence: seq,
            emitted_at: SystemTime::now(),
            payload: serde_json::json!({ "seq": seq }),
        }
    }

    #[test]
    fn enforces_capacity_and_tracks_drops() {
        let mut queue = InMemoryQueue::new(1, 1, 1);
        queue.enqueue_action(dummy_action("a1"));
        queue.enqueue_action(dummy_action("a2"));
        queue.enqueue_ack(dummy_ack("ack1"));
        queue.enqueue_ack(dummy_ack("ack2"));
        queue.enqueue_state(dummy_state(1));
        queue.enqueue_state(dummy_state(2));

        let stats = queue.stats();
        assert_eq!(stats.actions, 1);
        assert_eq!(stats.acks, 1);
        assert_eq!(stats.states, 1);
        assert_eq!(stats.drops_actions, 1);
        assert_eq!(stats.drops_acks, 1);
        assert_eq!(stats.drops_states, 1);
    }

    #[test]
    fn drains_up_to_max_items() {
        let mut queue = InMemoryQueue::new(5, 5, 5);
        queue.enqueue_action(dummy_action("a1"));
        queue.enqueue_action(dummy_action("a2"));
        queue.enqueue_ack(dummy_ack("ack1"));
        queue.enqueue_state(dummy_state(1));
        queue.enqueue_state(dummy_state(2));

        let actions = queue.drain_actions(1);
        assert_eq!(actions.len(), 1);
        assert_eq!(queue.stats().actions, 1);

        let states = queue.drain_states(10);
        assert_eq!(states.len(), 2);
        assert_eq!(queue.stats().states, 0);

        let acks = queue.drain_acks(1);
        assert_eq!(acks.len(), 1);
        assert_eq!(queue.stats().acks, 0);
    }
}

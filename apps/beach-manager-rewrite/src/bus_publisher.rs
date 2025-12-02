use std::sync::Arc;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use transport_bus::{Bus, BusResult};

use crate::queue::{ActionAck, ActionCommand, StateDiff};

const TOPIC_ACTION: &str = "beach.manager.action";
#[allow(dead_code)]
const TOPIC_ACK: &str = "beach.manager.ack";
#[allow(dead_code)]
const TOPIC_STATE: &str = "beach.manager.state";
#[allow(dead_code)]
const TOPIC_HEALTH: &str = "beach.manager.health";

#[derive(Serialize, Deserialize)]
struct Envelope<T> {
    #[serde(rename = "type")]
    kind: String,
    payload: T,
}

/// Manager-facing publisher for unified bus topics.
#[derive(Clone)]
pub struct ManagerBusPublisher<B: Bus + 'static> {
    bus: Arc<B>,
}

impl<B: Bus + 'static> ManagerBusPublisher<B> {
    pub fn new(bus: Arc<B>) -> Self {
        Self { bus }
    }

    pub fn publish_action(&self, cmd: ActionCommand) -> BusResult<()> {
        self.publish(TOPIC_ACTION, "action", cmd)
    }

    #[allow(dead_code)]
    pub fn publish_ack(&self, ack: ActionAck) -> BusResult<()> {
        self.publish(TOPIC_ACK, "ack", ack)
    }

    #[allow(dead_code)]
    pub fn publish_state(&self, state: StateDiff) -> BusResult<()> {
        self.publish(TOPIC_STATE, "state", state)
    }

    #[allow(dead_code)]
    pub fn publish_health(&self, payload: serde_json::Value) -> BusResult<()> {
        self.publish(TOPIC_HEALTH, "health", payload)
    }

    fn publish<T: Serialize>(&self, topic: &str, kind: &'static str, payload: T) -> BusResult<()> {
        let env = Envelope {
            kind: kind.to_string(),
            payload,
        };
        let bytes = serde_json::to_vec(&env)
            .map_err(|e| transport_bus::BusError::Transport(e.to_string()))?;
        self.bus.publish(topic, Bytes::from(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use transport_bus::LocalBus;

    #[tokio::test]
    async fn publishes_action_envelope() {
        let bus = Arc::new(LocalBus::new());
        let mut sub = bus.subscribe(TOPIC_ACTION);
        let publisher = ManagerBusPublisher::new(bus.clone());
        publisher
            .publish_action(ActionCommand {
                id: "a1".into(),
                action_type: "write".into(),
                payload: serde_json::json!({"bytes": "data"}),
            })
            .expect("publish ok");

        let msg = sub.recv().await.expect("recv");
        assert_eq!(msg.topic, TOPIC_ACTION);
        let payload = msg.payload.to_vec();
        let env: Envelope<ActionCommand> = serde_json::from_slice(&payload).unwrap();
        assert_eq!(env.kind, "action");
        assert_eq!(env.payload.id, "a1");
    }
}

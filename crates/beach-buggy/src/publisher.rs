use std::sync::Arc;

use bytes::Bytes;
use transport_bus::Bus;

use crate::{ActionAck, ActionCommand, HarnessError, HarnessResult, HealthHeartbeat, StateDiff};

pub const TOPIC_CONTROLLER_INPUT: &str = "beach.manager.action";
pub const TOPIC_CONTROLLER_ACK: &str = "beach.manager.ack";
pub const TOPIC_CONTROLLER_STATE: &str = "beach.manager.state";
pub const TOPIC_CONTROLLER_HEALTH: &str = "beach.manager.health";

pub struct ControllerBusPublisher<B: Bus> {
    bus: Arc<B>,
}

impl<B: Bus> ControllerBusPublisher<B> {
    pub fn new(bus: Arc<B>) -> Self {
        Self { bus }
    }

    pub fn publish_action(&self, action: &ActionCommand) -> HarnessResult<()> {
        self.publish(TOPIC_CONTROLLER_INPUT, "action", action)
    }

    pub fn publish_ack(&self, ack: &ActionAck) -> HarnessResult<()> {
        self.publish(TOPIC_CONTROLLER_ACK, "ack", ack)
    }

    pub fn publish_state(&self, diff: &StateDiff) -> HarnessResult<()> {
        self.publish(TOPIC_CONTROLLER_STATE, "state", diff)
    }

    pub fn publish_health(&self, heartbeat: &HealthHeartbeat) -> HarnessResult<()> {
        self.publish(TOPIC_CONTROLLER_HEALTH, "health", heartbeat)
    }

    fn publish<T: serde::Serialize>(
        &self,
        topic: &str,
        kind: &str,
        payload: &T,
    ) -> HarnessResult<()> {
        let envelope = serde_json::to_vec(&serde_json::json!({
            "type": kind,
            "payload": payload
        }))
        .map_err(|err| HarnessError::Transport(err.to_string()))?;
        self.bus
            .publish(topic, Bytes::from(envelope))
            .map_err(|err| HarnessError::Transport(err.to_string()))
    }
}

impl<B: Bus> Clone for ControllerBusPublisher<B> {
    fn clone(&self) -> Self {
        Self {
            bus: Arc::clone(&self.bus),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use transport_bus::LocalBus;

    #[test]
    fn publishes_enveloped_messages() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let bus = Arc::new(LocalBus::new());
        let publisher = ControllerBusPublisher::new(bus.clone());
        let mut rx = bus.subscribe(TOPIC_CONTROLLER_INPUT);

        let action = ActionCommand {
            id: "a-1".into(),
            action_type: "terminal_write".into(),
            payload: serde_json::json!({"bytes": "ls"}),
            expires_at: None,
        };

        rt.block_on(async move {
            publisher.publish_action(&action).expect("publish ok");
            let msg = rx.recv().await.expect("msg");
            assert_eq!(msg.topic, TOPIC_CONTROLLER_INPUT);
            let value: serde_json::Value =
                serde_json::from_slice(&msg.payload).expect("valid json envelope");
            assert_eq!(value["type"], "action");
            assert_eq!(value["payload"]["id"], "a-1");
        });
    }
}

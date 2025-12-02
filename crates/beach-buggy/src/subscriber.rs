use std::sync::Arc;
use std::time::SystemTime;

use tracing::{debug, warn};
use transport_bus::Bus;

use crate::fast_path::parse_action_payload;
use crate::publisher::{ControllerBusPublisher, TOPIC_CONTROLLER_ACK, TOPIC_CONTROLLER_INPUT};
use crate::{AckStatus, ActionAck, ActionCommand};

pub trait TerminalWriter: Send + Sync {
    fn write(&self, bytes: &[u8]) -> Result<(), String>;
}

impl<F> TerminalWriter for F
where
    F: Fn(&[u8]) -> Result<(), String> + Send + Sync,
{
    fn write(&self, bytes: &[u8]) -> Result<(), String> {
        (self)(bytes)
    }
}

#[derive(Clone)]
pub struct ControllerBusSubscriber<B: Bus> {
    bus: Arc<B>,
}

impl<B: Bus + 'static> ControllerBusSubscriber<B> {
    pub fn new(bus: Arc<B>) -> Self {
        Self { bus }
    }

    /// Subscribes to controller inputs, writes them to the supplied terminal writer,
    /// and publishes acknowledgements on the bus.
    pub fn spawn_terminal_worker<W: TerminalWriter + 'static>(
        &self,
        writer: W,
        publisher: ControllerBusPublisher<B>,
    ) -> tokio::task::JoinHandle<()> {
        let mut rx = self.bus.subscribe(TOPIC_CONTROLLER_INPUT);
        let writer = Arc::new(writer);
        tokio::spawn(async move {
            while let Ok(msg) = rx.recv().await {
                let ack = match std::str::from_utf8(&msg.payload) {
                    Ok(text) => match parse_action_payload(text) {
                        Ok(action) => handle_action(writer.as_ref(), action),
                        Err(err) => {
                            warn!(target = "controller.bus", error = %err, "failed to decode controller action");
                            None
                        }
                    },
                    Err(err) => {
                        warn!(target = "controller.bus", error = %err, "invalid utf8 for controller input");
                        None
                    }
                };

                if let Some(ack) = ack {
                    if let Err(err) = publisher.publish_ack(&ack) {
                        warn!(
                            target = "controller.bus",
                            error = %err,
                            topic = TOPIC_CONTROLLER_ACK,
                            "failed to publish controller ack"
                        );
                    } else {
                        debug!(
                            target = "controller.bus",
                            action_id = %ack.id,
                            status = ?ack.status,
                            topic = TOPIC_CONTROLLER_ACK,
                            "published controller ack"
                        );
                    }
                }
            }
        })
    }
}

fn handle_action<W: TerminalWriter>(writer: &W, action: ActionCommand) -> Option<ActionAck> {
    let payload_bytes = match extract_terminal_bytes(&action) {
        Ok(bytes) => bytes,
        Err(err) => {
            warn!(
                target = "controller.bus",
                action_id = %action.id,
                error = %err,
                "controller action rejected"
            );
            return Some(ActionAck {
                id: action.id,
                status: AckStatus::Rejected,
                applied_at: SystemTime::now(),
                latency_ms: None,
                error_code: Some("unsupported_action".into()),
                error_message: Some(err),
            });
        }
    };

    let status = match writer.write(&payload_bytes) {
        Ok(_) => AckStatus::Ok,
        Err(err) => {
            warn!(
                target = "controller.bus",
                error = %err,
                action_id = %action.id,
                "terminal write failed for controller action"
            );
            AckStatus::Rejected
        }
    };

    Some(ActionAck {
        id: action.id,
        status,
        applied_at: SystemTime::now(),
        latency_ms: None,
        error_code: None,
        error_message: None,
    })
}

fn extract_terminal_bytes(action: &ActionCommand) -> Result<Vec<u8>, String> {
    if action.action_type.as_str() != "terminal_write" {
        return Err(format!("unsupported action type {}", action.action_type));
    }
    let bytes = action
        .payload
        .get("bytes")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "terminal_write payload missing bytes".to_string())?;
    Ok(bytes.as_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publisher::ControllerBusPublisher;
    use std::sync::Mutex;
    use tokio::time::Duration;
    use transport_bus::LocalBus;

    #[tokio::test]
    async fn writes_actions_and_publishes_acks() {
        let bus = Arc::new(LocalBus::new());
        let publisher = ControllerBusPublisher::new(bus.clone());
        let subscriber = ControllerBusSubscriber::new(bus.clone());

        let writes: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let writer = {
            let writes = writes.clone();
            move |bytes: &[u8]| {
                writes
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(bytes).into_owned());
                Ok(())
            }
        };

        let _task = subscriber.spawn_terminal_worker(writer, publisher.clone());
        let mut ack_rx = bus.subscribe(TOPIC_CONTROLLER_ACK);

        let action = ActionCommand {
            id: "task-1".into(),
            action_type: "terminal_write".into(),
            payload: serde_json::json!({"bytes": "echo hi"}),
            expires_at: None,
        };

        publisher.publish_action(&action).expect("publish action");

        let ack_msg = tokio::time::timeout(Duration::from_secs(2), ack_rx.recv())
            .await
            .expect("ack timeout")
            .expect("ack message");
        let envelope: serde_json::Value =
            serde_json::from_slice(&ack_msg.payload).expect("ack envelope");
        assert_eq!(envelope["type"], "ack");
        assert_eq!(envelope["payload"]["id"], "task-1");
        assert_eq!(envelope["payload"]["status"], "ok");

        let seen = writes.lock().unwrap();
        assert_eq!(seen.as_slice(), ["echo hi"]);
    }
}

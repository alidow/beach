use bytes::Bytes;
use thiserror::Error;
use tokio::sync::broadcast;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BusMessage {
    pub topic: String,
    pub payload: Bytes,
}

#[derive(Debug, Error)]
pub enum BusError {
    #[error("bus channel closed")]
    Closed,
    #[error("bus transport error: {0}")]
    Transport(String),
}

pub type BusResult<T> = Result<T, BusError>;

pub trait Bus: Send + Sync {
    fn subscribe(&self, topic: &str) -> broadcast::Receiver<BusMessage>;
    fn publish(&self, topic: &str, payload: Bytes) -> BusResult<()>;
}

/// Simple in-memory bus for tests and non-transport contexts.
#[derive(Debug, Default)]
pub struct LocalBus {
    topics: parking_lot::RwLock<std::collections::HashMap<String, broadcast::Sender<BusMessage>>>,
}

impl LocalBus {
    pub fn new() -> Self {
        Self::default()
    }

    fn sender_for(&self, topic: &str) -> broadcast::Sender<BusMessage> {
        let mut guard = self.topics.write();
        guard
            .entry(topic.to_string())
            .or_insert_with(|| broadcast::channel(64).0)
            .clone()
    }
}

impl Bus for LocalBus {
    fn subscribe(&self, topic: &str) -> broadcast::Receiver<BusMessage> {
        self.sender_for(topic).subscribe()
    }

    fn publish(&self, topic: &str, payload: Bytes) -> BusResult<()> {
        let sender = self.sender_for(topic);
        sender
            .send(BusMessage {
                topic: topic.to_string(),
                payload,
            })
            .map(|_| ())
            .map_err(|_| BusError::Closed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_bus_round_trip() {
        let bus = LocalBus::new();
        let mut sub = bus.subscribe("controller/input");
        bus.publish("controller/input", Bytes::from_static(b"ping"))
            .expect("publish ok");
        let msg = sub.recv().await.expect("receive ok");
        assert_eq!(msg.topic, "controller/input");
        assert_eq!(msg.payload, Bytes::from_static(b"ping"));
    }
}

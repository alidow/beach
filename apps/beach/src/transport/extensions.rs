use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use tokio::sync::broadcast;

use crate::protocol::ExtensionFrame;
use crate::transport::TransportId;

type Namespace = String;

static EXTENSION_TOPICS: LazyLock<
    RwLock<HashMap<TransportId, HashMap<Namespace, broadcast::Sender<ExtensionFrame>>>>,
> = LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn subscribe(id: TransportId, namespace: &str) -> broadcast::Receiver<ExtensionFrame> {
    let sender = {
        let mut guard = EXTENSION_TOPICS
            .write()
            .expect("extension topic lock poisoned");
        guard
            .entry(id)
            .or_default()
            .entry(namespace.to_string())
            .or_insert_with(|| broadcast::channel(128).0)
            .clone()
    };
    sender.subscribe()
}

pub fn publish(id: TransportId, frame: ExtensionFrame) {
    if let Some(namespace_map) = EXTENSION_TOPICS
        .read()
        .expect("extension topic lock poisoned")
        .get(&id)
    {
        if let Some(sender) = namespace_map.get(&frame.namespace) {
            let _ = sender.send(frame);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::TransportId;

    #[test]
    fn publishes_to_namespace_subscribers() {
        let id = TransportId(1);
        let mut rx = subscribe(id, "fastpath");
        let frame = ExtensionFrame {
            namespace: "fastpath".into(),
            kind: "action".into(),
            payload: bytes::Bytes::from_static(b"ping"),
        };
        publish(id, frame.clone());
        let received = rx.try_recv().expect("frame delivered");
        assert_eq!(received, frame);
    }

    #[test]
    fn isolates_namespaces() {
        let id = TransportId(2);
        let mut rx = subscribe(id, "fastpath");
        let frame = ExtensionFrame {
            namespace: "other".into(),
            kind: "noop".into(),
            payload: bytes::Bytes::from_static(b"noop"),
        };
        publish(id, frame);
        assert!(
            rx.try_recv().is_err(),
            "unexpected cross-namespace delivery"
        );
    }
}

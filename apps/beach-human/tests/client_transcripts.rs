use std::sync::Arc;
use std::time::Duration;

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use beach_human::client::terminal::TerminalClient;
use beach_human::transport::{Transport, TransportKind, TransportPair};
use serde_json::json;

fn send_text(transport: &dyn Transport, value: serde_json::Value) {
    let text = serde_json::to_string(&value).expect("serialize frame");
    transport.send_text(&text).expect("send frame");
}

#[tokio::test]
async fn client_replays_basic_snapshot() {
    let pair = TransportPair::new(TransportKind::Ipc);
    let transport: Arc<dyn Transport> = Arc::from(pair.client);
    let server = pair.server;

    let handle = tokio::task::spawn_blocking(move || {
        let client = TerminalClient::new(transport).with_render(false);
        match client.run() {
            Ok(()) => {}
            Err(err) => panic!("client error: {err}"),
        }
    });

    send_text(
        &*server,
        json!({
            "type": "hello",
            "subscription": 1,
            "max_seq": 0,
            "config": {
                "snapshot_budgets": [],
                "delta_budget": 512,
                "heartbeat_ms": 250
            }
        }),
    );
    send_text(&*server, json!({"type": "grid", "rows": 4, "cols": 10}));
    send_text(
        &*server,
        json!({
            "type": "snapshot",
            "subscription": 1,
            "lane": "foreground",
            "watermark": 1,
            "has_more": false,
            "updates": [
                {"kind": "row", "row": 0, "seq": 1, "text": "hello"},
                {"kind": "row", "row": 1, "seq": 2, "text": "world"}
            ]
        }),
    );
    send_text(
        &*server,
        json!({
            "type": "snapshot_complete",
            "subscription": 1,
            "lane": "foreground"
        }),
    );
    send_text(&*server, json!({"type": "shutdown"}));

    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("client join timeout")
        .expect("client thread");
}

#[tokio::test]
async fn client_applies_deltas() {
    let pair = TransportPair::new(TransportKind::Ipc);
    let transport: Arc<dyn Transport> = Arc::from(pair.client);
    let server = pair.server;

    let handle = tokio::task::spawn_blocking(move || {
        let client = TerminalClient::new(transport).with_render(false);
        let _ = client.run();
    });

    send_text(
        &*server,
        json!({
            "type": "hello",
            "subscription": 1,
            "max_seq": 0,
            "config": {
                "snapshot_budgets": [],
                "delta_budget": 512,
                "heartbeat_ms": 250
            }
        }),
    );
    send_text(&*server, json!({"type": "grid", "rows": 2, "cols": 5}));
    send_text(
        &*server,
        json!({
            "type": "snapshot",
            "subscription": 1,
            "lane": "foreground",
            "watermark": 1,
            "has_more": false,
            "updates": [
                {"kind": "row", "row": 0, "seq": 1, "text": "hello"}
            ]
        }),
    );
    send_text(
        &*server,
        json!({
            "type": "snapshot_complete",
            "subscription": 1,
            "lane": "foreground"
        }),
    );
    send_text(
        &*server,
        json!({
            "type": "delta",
            "subscription": 1,
            "watermark": 2,
            "has_more": false,
            "updates": [
                {"kind": "cell", "row": 0, "col": 4, "seq": 2, "char": "!"}
            ]
        }),
    );
    send_text(&*server, json!({"type": "shutdown"}));

    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("client join timeout")
        .expect("client thread");
}

#[tokio::test]
async fn client_emits_input_events() {
    let pair = TransportPair::new(TransportKind::Ipc);
    let transport: Arc<dyn Transport> = Arc::from(pair.client);
    let server = pair.server;

    let (tx, rx) = std::sync::mpsc::channel();

    let handle = tokio::task::spawn_blocking(move || {
        let client = TerminalClient::new(transport)
            .with_render(false)
            .with_input(rx);
        let _ = client.run();
    });

    send_text(
        &*server,
        json!({
            "type": "hello",
            "subscription": 1,
            "max_seq": 0,
            "config": {
                "snapshot_budgets": [],
                "delta_budget": 512,
                "heartbeat_ms": 250
            }
        }),
    );
    send_text(&*server, json!({"type": "grid", "rows": 1, "cols": 5}));

    tx.send(b"a".to_vec()).expect("send input");

    let message = server
        .recv(Duration::from_secs(1))
        .expect("receive input frame");
    let text = message.payload.as_text().expect("text payload");
    let value: serde_json::Value = serde_json::from_str(text).expect("json frame");
    assert_eq!(value["type"], "input");
    let data = value["data"].as_str().expect("data field");
    let decoded = BASE64.decode(data.as_bytes()).expect("decode payload");
    assert_eq!(decoded, b"a");

    send_text(&*server, json!({"type": "shutdown"}));

    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("client join timeout")
        .expect("client thread");
}

use std::sync::Arc;
use std::time::Duration;

use beach_manager_rewrite::bus_ingest;
use beach_manager_rewrite::persistence::InMemoryPersistence;
use beach_manager_rewrite::pipeline;
use beach_manager_rewrite::queue::{ActionCommand, InMemoryQueue};
use transport_unified_adapter::UnifiedBusAdapter;
use transport_webrtc::RtcUnifiedAdapter;

fn env_or_empty(key: &str) -> String {
    std::env::var(key).unwrap_or_default()
}

#[tokio::test]
#[ignore = "requires BEACH_RTC_TEST_HOST_SESSION_ID + BEACH_SESSION_SERVER_BASE/BEACH_ROAD_URL and a reachable host"]
async fn rtc_bus_ingest_persists_action() {
    let host_session_id = match std::env::var("BEACH_RTC_TEST_HOST_SESSION_ID") {
        Ok(id) if !id.trim().is_empty() => id,
        _ => return,
    };
    let session_base = env_or_empty("BEACH_ROAD_URL");
    let session_base = if session_base.trim().is_empty() {
        env_or_empty("BEACH_SESSION_SERVER_BASE")
    } else {
        session_base
    };
    if session_base.trim().is_empty() {
        return;
    }

    let adapter = RtcUnifiedAdapter::new(session_base);
    let bus = match adapter.build_bus(&host_session_id).await {
        Ok(bus) => bus,
        Err(err) => {
            eprintln!("rtc attach failed: {err}");
            return;
        }
    };

    let queue = Arc::new(tokio::sync::Mutex::new(InMemoryQueue::new()));
    let persistence = InMemoryPersistence::new();
    let handles = bus_ingest::start_bus_ingest(bus.clone(), queue.clone());

    let action = ActionCommand {
        id: "rtc-action".into(),
        action_type: "write".into(),
        payload: serde_json::json!({
            "host_session_id": host_session_id,
            "controller_session_id": "rtc-controller",
            "bytes": "hi over rtc"
        }),
    };
    let payload = serde_json::to_vec(&serde_json::json!({
        "type": "action",
        "payload": action,
    }))
    .expect("serialize action payload");
    let _ = bus.publish("beach.manager.action", payload.into());

    tokio::time::sleep(Duration::from_millis(400)).await;
    pipeline::drain_once(queue.clone(), persistence.clone(), 16).await;
    let actions = persistence.actions().await;
    assert!(
        actions.iter().any(|record| record.id == "rtc-action"),
        "expected action to persist via rtc bus ingest"
    );

    for handle in handles {
        handle.abort();
    }
}

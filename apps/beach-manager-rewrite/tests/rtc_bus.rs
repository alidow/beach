use std::sync::Arc;
use std::time::Duration;

use beach_manager_rewrite::bus_ingest;
use beach_manager_rewrite::persistence::InMemoryPersistence;
use beach_manager_rewrite::pipeline;
use beach_manager_rewrite::queue::{ActionCommand, InMemoryQueue};
use tokio::time::timeout;
use transport_unified_adapter::UnifiedBusAdapter;
use transport_webrtc::RtcUnifiedAdapter;

fn env_or_empty(key: &str) -> String {
    std::env::var(key).unwrap_or_default()
}

#[tokio::test]
#[ignore = "requires BEACH_RTC_TEST_HOST_SESSION_ID + BEACH_SESSION_SERVER_BASE/BEACH_ROAD_URL and a reachable host"]
async fn rtc_bus_ingest_persists_action() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
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
    let bus = timeout(Duration::from_secs(20), adapter.build_bus(&host_session_id))
        .await
        .unwrap_or_else(|_| panic!("rtc attach timed out for host {host_session_id}"))
        .unwrap_or_else(|err| panic!("rtc attach failed: {err}"));
    tracing::info!("rtc bus connected for host {host_session_id}");

    let queue = Arc::new(tokio::sync::Mutex::new(InMemoryQueue::new()));
    let persistence = InMemoryPersistence::new();
    let handles = bus_ingest::start_bus_ingest(bus.clone(), queue.clone());
    tracing::info!("bus ingest running");

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
    bus.publish("beach.manager.action", payload.clone().into())
        .unwrap_or_else(|err| panic!("publish over rtc bus failed: {err}"));
    tracing::info!("published action over rtc bus");

    tokio::time::sleep(Duration::from_millis(400)).await;
    pipeline::drain_once(queue.clone(), persistence.clone(), 16, None).await;
    tracing::info!("drained pipeline once");
    let actions = persistence.actions().await;
    assert!(
        actions.iter().any(|record| record.id == "rtc-action"),
        "expected action to persist via rtc bus ingest"
    );

    for handle in handles {
        handle.abort();
    }
    // Tokio keeps WebRTC background tasks alive; force a clean exit for this manual smoke.
    std::process::exit(0);
}

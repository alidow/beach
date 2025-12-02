use std::sync::Arc;

use beach_manager_rewrite::bus_ingest;
use beach_manager_rewrite::bus_publisher::ManagerBusPublisher;
use beach_manager_rewrite::persistence::InMemoryPersistence;
use beach_manager_rewrite::pipeline;
use beach_manager_rewrite::queue::InMemoryQueue;
use transport_bus::LocalBus;

/// Exercises bus ingest + pipeline using the IPC/unified bus placeholder.
#[tokio::test]
async fn ipc_bus_to_persistence_round_trip() {
    let queue = Arc::new(tokio::sync::Mutex::new(InMemoryQueue::new()));
    let persistence = InMemoryPersistence::new();
    let mgr_bus: Arc<LocalBus> = Arc::new(LocalBus::new());
    let client_bus = mgr_bus.clone();
    let _handles = bus_ingest::start_bus_ingest(mgr_bus.clone(), queue.clone());

    let publisher = ManagerBusPublisher::new(client_bus.clone());
    publisher
        .publish_action(beach_manager_rewrite::queue::ActionCommand {
            id: "ipc-act".into(),
            action_type: "write".into(),
            payload: serde_json::json!({"host_session_id": "ipc-host", "controller_session_id": "ipc-ctrl"}),
        })
        .expect("publish action");

    // Give the bus a moment and drain to persistence.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    pipeline::drain_once(queue.clone(), persistence.clone(), 16).await;
    let actions = persistence.actions().await;
    assert!(actions.iter().any(|a| a.id == "ipc-act"));
}

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use beach_manager_rewrite::assignment::AssignmentService;
use beach_manager_rewrite::assignment_orm::SeaOrmAssignmentStore;
use beach_manager_rewrite::assignment_postgres::PostgresAssignmentStore;
use beach_manager_rewrite::assignment_redis::RedisAssignmentStore;
use manager_sdk::assignment_store::{
    AssignmentStore, InMemoryAssignmentStore, ManagerInstanceRecord,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn build_store() -> Arc<dyn AssignmentStore> {
    if let Ok(url) = std::env::var("REDIS_URL") {
        if let Ok(store) = RedisAssignmentStore::connect(&url) {
            return Arc::new(store);
        }
    }
    if let Ok(url) = std::env::var("PG_URL").or_else(|_| std::env::var("DATABASE_URL")) {
        if let Ok(store) = SeaOrmAssignmentStore::connect(&url).await {
            return Arc::new(store);
        }
        if let Ok(store) = PostgresAssignmentStore::connect(&url).await {
            return Arc::new(store);
        }
    }
    InMemoryAssignmentStore::new()
}

fn unique_id(name: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    format!("{name}-{millis}")
}

#[tokio::test]
#[ignore]
async fn reassigns_stale_and_respects_capacity() -> TestResult {
    let store = build_store().await;
    let ttl_ms = 50;

    let stale_id = unique_id("mgr-stale");
    let fresh_id = unique_id("mgr-fresh");
    let open_id = unique_id("mgr-open");

    let stale_mgr = AssignmentService::from_store(store.clone(), stale_id.clone(), 1, ttl_ms);
    let fresh_mgr = AssignmentService::from_store(store.clone(), fresh_id.clone(), 1, ttl_ms);
    let open_mgr = AssignmentService::from_store(store.clone(), open_id.clone(), 5, ttl_ms);

    // Initial assignment goes to the stale manager (only live instance).
    let initial = stale_mgr
        .assign_host("host-a")
        .await?
        .expect("initial assignment");
    assert_eq!(initial.selected.id, stale_id);

    // Wait long enough for the stale manager heartbeat to be considered expired.
    tokio::time::sleep(Duration::from_millis(ttl_ms as u64 + 25)).await;

    // Register a fresh manager and ensure reassignment away from the stale one.
    fresh_mgr.register_self().await?;
    let reassigned = fresh_mgr
        .assign_host("host-a")
        .await?
        .expect("reassigned host");
    assert_eq!(reassigned.selected.id, fresh_id);
    assert_eq!(
        reassigned.reassigned_from.as_deref(),
        Some(stale_id.as_str())
    );
    assert!(reassigned.assigned_here);

    // Mark the stale heartbeat explicitly to avoid it showing up if the backend keeps it around.
    store
        .upsert_instance(ManagerInstanceRecord {
            id: stale_id.clone(),
            capacity: 1,
            load: 1,
            heartbeat_at: SystemTime::UNIX_EPOCH,
        })
        .await?;

    // Register an open manager. The fresh manager is now at capacity (1/1) and should be skipped.
    open_mgr.register_self().await?;
    let rerouted = fresh_mgr
        .assign_host("host-b")
        .await?
        .expect("rerouted host");
    assert_eq!(rerouted.selected.id, open_id);
    assert!(!rerouted.assigned_here);

    Ok(())
}

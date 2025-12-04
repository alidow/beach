use std::time::SystemTime;

use beach_manager_rewrite::persistence::{
    ActionLogRecord, ControllerLeaseRecord, ManagerAssignmentRecord, PersistenceAdapter,
    SeaOrmPersistence,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
#[ignore]
async fn postgres_persistence_roundtrip() -> TestResult {
    let url = match std::env::var("DATABASE_URL").or_else(|_| std::env::var("PG_URL")) {
        Ok(url) if !url.trim().is_empty() => url,
        _ => return Ok(()),
    };
    let store = SeaOrmPersistence::connect(&url).await?;

    store
        .upsert_controller_lease(ControllerLeaseRecord {
            host_session_id: "pg-host".into(),
            controller_session_id: "pg-ctrl".into(),
            lease_id: "pg-lease".into(),
            expires_at: SystemTime::now(),
        })
        .await?;

    store
        .append_action_log(ActionLogRecord {
            id: "pg-action".into(),
            host_session_id: "pg-host".into(),
            controller_session_id: "pg-ctrl".into(),
            action_type: "write".into(),
            payload: serde_json::json!({"bytes": "hi pg"}),
            emitted_at: SystemTime::now(),
        })
        .await?;

    store
        .record_assignment(ManagerAssignmentRecord {
            host_session_id: "pg-host".into(),
            manager_instance_id: "pg-manager".into(),
            assigned_at: SystemTime::now(),
            reassigned_from: None,
        })
        .await?;

    Ok(())
}

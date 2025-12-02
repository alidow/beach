use std::sync::Arc;
use std::time::Duration;

use tracing::warn;

use crate::persistence::{
    ActionLogRecord, ControllerLeaseRecord, ManagerAssignmentRecord, PersistenceAdapter,
};
use crate::queue::ControllerQueue;

pub async fn drain_once(
    queue: Arc<dyn ControllerQueue>,
    persistence: Arc<dyn PersistenceAdapter>,
    batch: usize,
) {
    let actions = queue.drain_actions(batch).await;
    let acks = queue.drain_acks(batch).await;
    let states = queue.drain_states(batch).await;

    for action in actions {
        let record = ActionLogRecord {
            id: action.id.clone(),
            host_session_id: action
                .payload
                .get("host_session_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            controller_session_id: action
                .payload
                .get("controller_session_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            action_type: action.action_type,
            payload: action.payload,
            emitted_at: std::time::SystemTime::now(),
        };
        if let Err(err) = persistence.append_action_log(record).await {
            warn!(error = %err, "persist action failed");
        }
    }

    for ack in acks {
        let lease = ControllerLeaseRecord {
            host_session_id: "".into(),
            controller_session_id: "".into(),
            lease_id: ack.id,
            expires_at: ack.applied_at,
        };
        if let Err(err) = persistence.upsert_controller_lease(lease).await {
            warn!(error = %err, "persist ack failed");
        }
    }

    for state in states {
        let record = ManagerAssignmentRecord {
            host_session_id: state
                .payload
                .get("host_session_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            manager_instance_id: state
                .payload
                .get("manager_instance_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            assigned_at: state.emitted_at,
            reassigned_from: None,
        };
        if let Err(err) = persistence.record_assignment(record).await {
            warn!(error = %err, "persist state failed");
        }
    }
}

pub fn start_pipeline(
    queue: Arc<dyn ControllerQueue>,
    persistence: Arc<dyn PersistenceAdapter>,
    batch: usize,
    interval_ms: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(interval_ms));
        loop {
            ticker.tick().await;
            drain_once(queue.clone(), persistence.clone(), batch).await;
        }
    })
}

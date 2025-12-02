use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use tokio::sync::Mutex;

/// Record of a manager instance for assignment.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ManagerInstanceRecord {
    pub id: String,
    pub capacity: u32,
    pub load: u32,
    pub heartbeat_at: std::time::SystemTime,
}

/// Mapping of host_session_id to manager instance.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ManagerAssignmentRecord {
    pub host_session_id: String,
    pub manager_instance_id: String,
    pub assigned_at: std::time::SystemTime,
    pub reassigned_from: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum AssignmentStoreError {
    #[error("store error: {0}")]
    Store(String),
}

#[async_trait]
pub trait AssignmentStore: Send + Sync {
    async fn upsert_instance(
        &self,
        instance: ManagerInstanceRecord,
    ) -> Result<(), AssignmentStoreError>;

    async fn list_instances(&self) -> Result<Vec<ManagerInstanceRecord>, AssignmentStoreError>;

    /// List instances with heartbeat at or after the given cutoff. Defaults to filtering
    /// the full list in-memory; backends can override for server-side filtering.
    async fn live_instances_since(
        &self,
        cutoff: SystemTime,
    ) -> Result<Vec<ManagerInstanceRecord>, AssignmentStoreError> {
        let instances = self.list_instances().await?;
        Ok(instances
            .into_iter()
            .filter(|i| i.heartbeat_at >= cutoff)
            .collect())
    }

    async fn record_assignment(
        &self,
        record: ManagerAssignmentRecord,
    ) -> Result<(), AssignmentStoreError>;

    async fn list_assignments(&self) -> Result<Vec<ManagerAssignmentRecord>, AssignmentStoreError>;
}

/// In-memory adapter for tests and early wiring.
#[derive(Default)]
pub struct InMemoryAssignmentStore {
    instances: Mutex<Vec<ManagerInstanceRecord>>,
    assignments: Mutex<Vec<ManagerAssignmentRecord>>,
}

impl InMemoryAssignmentStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

#[async_trait]
impl AssignmentStore for InMemoryAssignmentStore {
    async fn upsert_instance(
        &self,
        instance: ManagerInstanceRecord,
    ) -> Result<(), AssignmentStoreError> {
        let mut guard = self.instances.lock().await;
        if let Some(existing) = guard.iter_mut().find(|i| i.id == instance.id) {
            *existing = instance;
        } else {
            guard.push(instance);
        }
        Ok(())
    }

    async fn list_instances(&self) -> Result<Vec<ManagerInstanceRecord>, AssignmentStoreError> {
        Ok(self.instances.lock().await.clone())
    }

    async fn record_assignment(
        &self,
        record: ManagerAssignmentRecord,
    ) -> Result<(), AssignmentStoreError> {
        self.assignments.lock().await.push(record);
        Ok(())
    }

    async fn list_assignments(&self) -> Result<Vec<ManagerAssignmentRecord>, AssignmentStoreError> {
        Ok(self.assignments.lock().await.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn upserts_and_lists_instances() {
        let store = InMemoryAssignmentStore::new();
        store
            .upsert_instance(ManagerInstanceRecord {
                id: "mgr-1".into(),
                capacity: 50,
                load: 10,
                heartbeat_at: std::time::SystemTime::now(),
            })
            .await
            .unwrap();
        store
            .upsert_instance(ManagerInstanceRecord {
                id: "mgr-1".into(),
                capacity: 60,
                load: 5,
                heartbeat_at: std::time::SystemTime::now(),
            })
            .await
            .unwrap();
        let instances = store.list_instances().await.unwrap();
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].capacity, 60);
        assert_eq!(instances[0].load, 5);
    }

    #[tokio::test]
    async fn records_assignments() {
        let store = InMemoryAssignmentStore::new();
        store
            .record_assignment(ManagerAssignmentRecord {
                host_session_id: "h1".into(),
                manager_instance_id: "mgr-1".into(),
                assigned_at: std::time::SystemTime::now(),
                reassigned_from: None,
            })
            .await
            .unwrap();
        let assignments = store.list_assignments().await.unwrap();
        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].manager_instance_id, "mgr-1");
    }
}

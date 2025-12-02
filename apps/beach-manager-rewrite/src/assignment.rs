use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::assignment_orm::SeaOrmAssignmentStore;
use crate::assignment_postgres::PostgresAssignmentStore;
use crate::assignment_redis::RedisAssignmentStore;
use crate::config::AppConfig;
use manager_sdk::assignment::{ManagerInstance, select_manager};
use manager_sdk::assignment_store::{
    AssignmentStore, AssignmentStoreError, InMemoryAssignmentStore, ManagerAssignmentRecord,
    ManagerInstanceRecord,
};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignmentDecision {
    pub selected: ManagerInstance,
    pub reassigned_from: Option<String>,
    pub assigned_here: bool,
}

#[derive(Clone)]
pub struct AssignmentService {
    store: Arc<dyn AssignmentStore>,
    instance_id: String,
    capacity: u32,
    ttl_ms: u64,
    local_assignments: Arc<Mutex<HashSet<String>>>,
}

impl AssignmentService {
    pub fn from_store(
        store: Arc<dyn AssignmentStore>,
        instance_id: String,
        capacity: u32,
        ttl_ms: u64,
    ) -> Self {
        Self {
            store,
            instance_id,
            capacity,
            ttl_ms,
            local_assignments: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub async fn build(
        redis_url: Option<&str>,
        pg_url: Option<&str>,
        instance_id: String,
        capacity: u32,
        ttl_ms: u64,
    ) -> Self {
        if let Some(url) = pg_url {
            if let Ok(store) = SeaOrmAssignmentStore::connect(url).await {
                return Self::from_store(Arc::new(store), instance_id, capacity, ttl_ms);
            }
            if let Ok(store) = PostgresAssignmentStore::connect(url).await {
                return Self::from_store(Arc::new(store), instance_id, capacity, ttl_ms);
            }
        }
        if let Some(url) = redis_url {
            if let Ok(store) = RedisAssignmentStore::connect(url) {
                return Self::from_store(Arc::new(store), instance_id, capacity, ttl_ms);
            }
        }
        Self::from_store(
            InMemoryAssignmentStore::new(),
            instance_id,
            capacity,
            ttl_ms,
        )
    }

    pub async fn build_assignment_service(cfg: &AppConfig) -> Self {
        Self::build(
            cfg.redis_url.as_deref(),
            cfg.database_url.as_deref(),
            cfg.manager_instance_id.clone(),
            cfg.manager_capacity,
            cfg.assignment_ttl_ms,
        )
        .await
    }

    fn cutoff(&self) -> SystemTime {
        SystemTime::now()
            .checked_sub(Duration::from_millis(self.ttl_ms))
            .unwrap_or(SystemTime::UNIX_EPOCH)
    }

    async fn current_load(&self) -> u32 {
        self.local_assignments.lock().await.len() as u32
    }

    async fn update_local_assignment(&self, host_session_id: &str, assigned_here: bool) -> bool {
        let mut guard = self.local_assignments.lock().await;
        if assigned_here {
            guard.insert(host_session_id.to_string())
        } else {
            guard.remove(host_session_id)
        }
    }

    pub async fn register_self(&self) -> Result<(), AssignmentStoreError> {
        let load = self.current_load().await;
        self.store
            .upsert_instance(ManagerInstanceRecord {
                id: self.instance_id.clone(),
                capacity: self.capacity,
                load,
                heartbeat_at: SystemTime::now(),
            })
            .await
    }

    pub async fn assign_host(
        &self,
        host_session_id: &str,
    ) -> Result<Option<AssignmentDecision>, AssignmentStoreError> {
        // Ensure self is present
        self.register_self().await?;
        let cutoff = self.cutoff();
        let instances = self.store.live_instances_since(cutoff).await?;
        let candidates: Vec<ManagerInstance> = instances
            .into_iter()
            .map(|i| ManagerInstance {
                id: i.id,
                capacity: i.capacity,
                load: i.load,
            })
            .collect();
        if candidates.is_empty() {
            debug!(
                host_session_id,
                "no live manager instances available for assignment"
            );
            return Ok(None);
        }

        let previous_assignment = self
            .store
            .list_assignments()
            .await?
            .into_iter()
            .find(|a| a.host_session_id == host_session_id);

        if let Some(selected) = select_manager(host_session_id, &candidates) {
            let reassigned_from = previous_assignment.as_ref().and_then(|p| {
                (p.manager_instance_id != selected.id).then(|| p.manager_instance_id.clone())
            });
            let assigned_here = selected.id == self.instance_id;
            let load_changed = self
                .update_local_assignment(host_session_id, assigned_here)
                .await;
            if load_changed {
                // Refresh the stored load so capacity-aware selection sees the change.
                if let Err(err) = self.register_self().await {
                    warn!(error = %err, "failed to refresh self registration after assignment");
                }
            }
            self.store
                .record_assignment(ManagerAssignmentRecord {
                    host_session_id: host_session_id.to_string(),
                    manager_instance_id: selected.id.clone(),
                    assigned_at: SystemTime::now(),
                    reassigned_from: reassigned_from.clone(),
                })
                .await?;
            Ok(Some(AssignmentDecision {
                selected,
                reassigned_from,
                assigned_here,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn spawn_heartbeat(&self, interval_ms: u64) -> JoinHandle<()> {
        let svc = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_millis(interval_ms));
            loop {
                ticker.tick().await;
                let _ = svc.register_self().await;
            }
        })
    }
}

#[cfg(test)]
pub fn in_memory_service(instance_id: String, capacity: u32) -> AssignmentService {
    let store = InMemoryAssignmentStore::new();
    AssignmentService::from_store(store, instance_id, capacity, 15_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn assigns_to_self_when_only_instance() {
        let svc = in_memory_service("mgr-self".into(), 50);
        let selected = svc.assign_host("host-1").await.unwrap().unwrap();
        assert_eq!(selected.selected.id, "mgr-self");
        assert!(selected.assigned_here);
    }

    #[tokio::test]
    async fn filters_stale_instances() {
        let store = InMemoryAssignmentStore::new();
        let svc = AssignmentService::from_store(store.clone(), "fresh".into(), 10, 5);
        // stale heartbeat
        store
            .upsert_instance(ManagerInstanceRecord {
                id: "stale".into(),
                capacity: 10,
                load: 0,
                heartbeat_at: std::time::SystemTime::UNIX_EPOCH,
            })
            .await
            .unwrap();
        // fresh heartbeat
        store
            .upsert_instance(ManagerInstanceRecord {
                id: "fresh".into(),
                capacity: 10,
                load: 0,
                heartbeat_at: std::time::SystemTime::now(),
            })
            .await
            .unwrap();
        let selected = svc.assign_host("host-2").await.unwrap().unwrap();
        assert_eq!(selected.selected.id, "fresh");
    }

    #[tokio::test]
    async fn skips_full_instances() {
        let store = InMemoryAssignmentStore::new();
        let svc = AssignmentService::from_store(store.clone(), "open".into(), 1, 15_000);
        // full instance should be ignored
        store
            .upsert_instance(ManagerInstanceRecord {
                id: "full".into(),
                capacity: 1,
                load: 1,
                heartbeat_at: SystemTime::now(),
            })
            .await
            .unwrap();
        // self is empty and should be picked
        store
            .upsert_instance(ManagerInstanceRecord {
                id: "open".into(),
                capacity: 1,
                load: 0,
                heartbeat_at: SystemTime::now(),
            })
            .await
            .unwrap();
        let selected = svc.assign_host("host-3").await.unwrap().unwrap();
        assert_eq!(selected.selected.id, "open");
    }
}

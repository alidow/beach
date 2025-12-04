use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::entity::prelude::*;
use sea_orm::sea_query::OnConflict;
use sea_orm::{Database, DatabaseConnection, EntityTrait, Set};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::warn;

use crate::assignment_orm::manager_assignments;
use crate::assignment_postgres::PostgresAssignmentStore;
use crate::config::AppConfig;
use crate::metrics::{PERSIST_ERROR, PERSIST_SUCCESS};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerLeaseRecord {
    pub host_session_id: String,
    pub controller_session_id: String,
    pub lease_id: String,
    pub expires_at: std::time::SystemTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionLogRecord {
    pub id: String,
    pub host_session_id: String,
    pub controller_session_id: String,
    pub action_type: String,
    pub payload: Value,
    pub emitted_at: std::time::SystemTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerAssignmentRecord {
    pub host_session_id: String,
    pub manager_instance_id: String,
    pub assigned_at: std::time::SystemTime,
    pub reassigned_from: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error("persistence backend error: {0}")]
    Backend(String),
}

#[async_trait]
pub trait PersistenceAdapter: Send + Sync {
    async fn upsert_controller_lease(
        &self,
        lease: ControllerLeaseRecord,
    ) -> Result<(), PersistenceError>;
    async fn append_action_log(&self, action: ActionLogRecord) -> Result<(), PersistenceError>;
    async fn record_assignment(
        &self,
        assignment: ManagerAssignmentRecord,
    ) -> Result<(), PersistenceError>;

    /// Optional hook for metrics to record errors; default no-op so adapters can override.
    fn record_error(&self, _kind: &str) {}
}

#[derive(Default)]
pub struct InMemoryPersistence {
    leases: tokio::sync::Mutex<Vec<ControllerLeaseRecord>>,
    actions: tokio::sync::Mutex<Vec<ActionLogRecord>>,
    assignments: tokio::sync::Mutex<Vec<ManagerAssignmentRecord>>,
}

impl InMemoryPersistence {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

impl InMemoryPersistence {
    pub async fn leases(&self) -> Vec<ControllerLeaseRecord> {
        self.leases.lock().await.clone()
    }

    pub async fn actions(&self) -> Vec<ActionLogRecord> {
        self.actions.lock().await.clone()
    }

    pub async fn assignments(&self) -> Vec<ManagerAssignmentRecord> {
        self.assignments.lock().await.clone()
    }
}

#[async_trait]
impl PersistenceAdapter for InMemoryPersistence {
    async fn upsert_controller_lease(
        &self,
        lease: ControllerLeaseRecord,
    ) -> Result<(), PersistenceError> {
        let mut leases = self.leases.lock().await;
        if let Some(existing) = leases.iter_mut().find(|l| {
            l.host_session_id == lease.host_session_id
                && l.controller_session_id == lease.controller_session_id
        }) {
            *existing = lease;
        } else {
            leases.push(lease);
        }
        Ok(())
    }

    async fn append_action_log(&self, action: ActionLogRecord) -> Result<(), PersistenceError> {
        self.actions.lock().await.push(action);
        Ok(())
    }

    async fn record_assignment(
        &self,
        assignment: ManagerAssignmentRecord,
    ) -> Result<(), PersistenceError> {
        self.assignments.lock().await.push(assignment);
        Ok(())
    }

    fn record_error(&self, _kind: &str) {}
}

pub fn build_persistence(_cfg: &AppConfig) -> Arc<dyn PersistenceAdapter> {
    if let Some(url) = _cfg.database_url.as_deref() {
        if let Ok(db) = SeaOrmPersistence::connect_sync(url) {
            return Arc::new(db);
        } else {
            warn!(
                "DATABASE_URL provided but Postgres adapter failed; falling back to Redis/memory"
            );
        }
    }
    if let Some(url) = _cfg.redis_url.as_deref() {
        if let Ok(redis) = RedisPersistence::connect(url) {
            return Arc::new(redis);
        }
    }
    // Postgres adapter TBD; start with in-memory for early wiring or when Redis unavailable.
    InMemoryPersistence::new()
}

/// Redis-backed persistence using simple hashes/lists.
pub struct RedisPersistence {
    client: redis::Client,
}

impl RedisPersistence {
    pub fn connect(url: &str) -> Result<Self, PersistenceError> {
        let client =
            redis::Client::open(url).map_err(|e| PersistenceError::Backend(e.to_string()))?;
        Ok(Self { client })
    }

    async fn conn(&self) -> Result<redis::aio::ConnectionManager, PersistenceError> {
        self.client
            .get_connection_manager()
            .await
            .map_err(|e| PersistenceError::Backend(e.to_string()))
    }
}

#[async_trait]
impl PersistenceAdapter for RedisPersistence {
    async fn upsert_controller_lease(
        &self,
        lease: ControllerLeaseRecord,
    ) -> Result<(), PersistenceError> {
        let mut conn = self.conn().await?;
        let key = format!(
            "mgr:lease:{}:{}",
            lease.host_session_id, lease.controller_session_id
        );
        let payload =
            serde_json::to_string(&lease).map_err(|e| PersistenceError::Backend(e.to_string()))?;
        redis::cmd("SET")
            .arg(&key)
            .arg(payload)
            .query_async::<_, ()>(&mut conn)
            .await
            .map_err(|e| {
                PERSIST_ERROR.with_label_values(&["lease"]).inc();
                PersistenceError::Backend(e.to_string())
            })?;
        PERSIST_SUCCESS.with_label_values(&["lease"]).inc();
        Ok(())
    }

    async fn append_action_log(&self, action: ActionLogRecord) -> Result<(), PersistenceError> {
        let mut conn = self.conn().await?;
        let payload =
            serde_json::to_string(&action).map_err(|e| PersistenceError::Backend(e.to_string()))?;
        redis::cmd("RPUSH")
            .arg("mgr:actions")
            .arg(payload)
            .query_async::<_, ()>(&mut conn)
            .await
            .map_err(|e| {
                PERSIST_ERROR.with_label_values(&["action"]).inc();
                PersistenceError::Backend(e.to_string())
            })?;
        PERSIST_SUCCESS.with_label_values(&["action"]).inc();
        Ok(())
    }

    async fn record_assignment(
        &self,
        assignment: ManagerAssignmentRecord,
    ) -> Result<(), PersistenceError> {
        let mut conn = self.conn().await?;
        let payload = serde_json::to_string(&assignment)
            .map_err(|e| PersistenceError::Backend(e.to_string()))?;
        redis::cmd("RPUSH")
            .arg("mgr:assignments")
            .arg(payload)
            .query_async::<_, ()>(&mut conn)
            .await
            .map_err(|e| {
                PERSIST_ERROR.with_label_values(&["assignment"]).inc();
                PersistenceError::Backend(e.to_string())
            })?;
        PERSIST_SUCCESS.with_label_values(&["assignment"]).inc();
        Ok(())
    }
}

/// SeaORM/Postgres-backed persistence for leases, actions, and assignments.
pub struct SeaOrmPersistence {
    conn: DatabaseConnection,
}

impl SeaOrmPersistence {
    pub fn connect_sync(url: &str) -> Result<Self, PersistenceError> {
        futures::executor::block_on(Self::connect(url))
    }

    pub async fn connect(url: &str) -> Result<Self, PersistenceError> {
        // Ensure all SQLx migrations have run (assignment tables + persistence tables).
        PostgresAssignmentStore::ensure_migrations(url)
            .await
            .map_err(|e| PersistenceError::Backend(e.to_string()))?;
        sqlx::migrate!()
            .run(
                &sqlx::postgres::PgPoolOptions::new()
                    .connect(url)
                    .await
                    .map_err(|e| PersistenceError::Backend(e.to_string()))?,
            )
            .await
            .map_err(|e| PersistenceError::Backend(e.to_string()))?;
        let conn = Database::connect(url)
            .await
            .map_err(|e| PersistenceError::Backend(e.to_string()))?;
        Ok(Self { conn })
    }
}

#[async_trait]
impl PersistenceAdapter for SeaOrmPersistence {
    async fn upsert_controller_lease(
        &self,
        lease: ControllerLeaseRecord,
    ) -> Result<(), PersistenceError> {
        let model = controller_leases::ActiveModel {
            lease_id: Set(lease.lease_id.clone()),
            host_session_id: Set(lease.host_session_id.clone()),
            controller_session_id: Set(lease.controller_session_id.clone()),
            expires_at: Set(crate::assignment_postgres::to_chrono(lease.expires_at).into()),
        };
        controller_leases::Entity::insert(model)
            .on_conflict(
                OnConflict::column(controller_leases::Column::LeaseId)
                    .update_columns([
                        controller_leases::Column::HostSessionId,
                        controller_leases::Column::ControllerSessionId,
                        controller_leases::Column::ExpiresAt,
                    ])
                    .to_owned(),
            )
            .exec(&self.conn)
            .await
            .map_err(|e| {
                PERSIST_ERROR.with_label_values(&["lease"]).inc();
                PersistenceError::Backend(e.to_string())
            })?;
        PERSIST_SUCCESS.with_label_values(&["lease"]).inc();
        Ok(())
    }

    async fn append_action_log(&self, action: ActionLogRecord) -> Result<(), PersistenceError> {
        let model = action_logs::ActiveModel {
            id: Set(action.id.clone()),
            host_session_id: Set(action.host_session_id.clone()),
            controller_session_id: Set(action.controller_session_id.clone()),
            action_type: Set(action.action_type.clone()),
            payload: Set(action.payload.to_string()),
            emitted_at: Set(crate::assignment_postgres::to_chrono(action.emitted_at).into()),
        };
        action_logs::Entity::insert(model)
            .on_conflict(
                OnConflict::column(action_logs::Column::Id)
                    .update_columns([
                        action_logs::Column::HostSessionId,
                        action_logs::Column::ControllerSessionId,
                        action_logs::Column::ActionType,
                        action_logs::Column::Payload,
                        action_logs::Column::EmittedAt,
                    ])
                    .to_owned(),
            )
            .exec(&self.conn)
            .await
            .map_err(|e| {
                PERSIST_ERROR.with_label_values(&["action"]).inc();
                PersistenceError::Backend(e.to_string())
            })?;
        PERSIST_SUCCESS.with_label_values(&["action"]).inc();
        Ok(())
    }

    async fn record_assignment(
        &self,
        assignment: ManagerAssignmentRecord,
    ) -> Result<(), PersistenceError> {
        let model = manager_assignments::ActiveModel {
            host_session_id: Set(assignment.host_session_id.clone()),
            manager_instance_id: Set(assignment.manager_instance_id.clone()),
            assigned_at: Set(crate::assignment_postgres::to_chrono(assignment.assigned_at).into()),
            reassigned_from: Set(assignment.reassigned_from.clone()),
        };
        manager_assignments::Entity::insert(model)
            .on_conflict(
                OnConflict::column(manager_assignments::Column::HostSessionId)
                    .update_columns([
                        manager_assignments::Column::ManagerInstanceId,
                        manager_assignments::Column::AssignedAt,
                        manager_assignments::Column::ReassignedFrom,
                    ])
                    .to_owned(),
            )
            .exec(&self.conn)
            .await
            .map_err(|e| {
                PERSIST_ERROR.with_label_values(&["assignment"]).inc();
                PersistenceError::Backend(e.to_string())
            })?;
        PERSIST_SUCCESS.with_label_values(&["assignment"]).inc();
        Ok(())
    }
}

mod controller_leases {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "controller_leases")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub lease_id: String,
        pub host_session_id: String,
        pub controller_session_id: String,
        pub expires_at: DateTimeWithTimeZone,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

mod action_logs {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "action_logs")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: String,
        pub host_session_id: String,
        pub controller_session_id: String,
        pub action_type: String,
        pub payload: String,
        pub emitted_at: DateTimeWithTimeZone,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[cfg(test)]
mod redis_tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn redis_persistence_roundtrip() {
        let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1/".into());
        let store = RedisPersistence::connect(&url).expect("redis");
        store
            .upsert_controller_lease(ControllerLeaseRecord {
                host_session_id: "h-redis".into(),
                controller_session_id: "c-redis".into(),
                lease_id: "lease-redis".into(),
                expires_at: std::time::SystemTime::now(),
            })
            .await
            .unwrap();
        store
            .append_action_log(ActionLogRecord {
                id: "act-redis".into(),
                host_session_id: "h-redis".into(),
                controller_session_id: "c-redis".into(),
                action_type: "write".into(),
                payload: serde_json::json!({"bytes": "hi"}),
                emitted_at: std::time::SystemTime::now(),
            })
            .await
            .unwrap();
        store
            .record_assignment(ManagerAssignmentRecord {
                host_session_id: "h-redis".into(),
                manager_instance_id: "mgr-redis".into(),
                assigned_at: std::time::SystemTime::now(),
                reassigned_from: None,
            })
            .await
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_round_trip() {
        let store = InMemoryPersistence::new();
        store
            .upsert_controller_lease(ControllerLeaseRecord {
                host_session_id: "h1".into(),
                controller_session_id: "c1".into(),
                lease_id: "lease-1".into(),
                expires_at: std::time::SystemTime::now(),
            })
            .await
            .unwrap();
        store
            .append_action_log(ActionLogRecord {
                id: "a1".into(),
                host_session_id: "h1".into(),
                controller_session_id: "c1".into(),
                action_type: "terminal_write".into(),
                payload: serde_json::json!({"bytes": "hi"}),
                emitted_at: std::time::SystemTime::now(),
            })
            .await
            .unwrap();
        store
            .record_assignment(ManagerAssignmentRecord {
                host_session_id: "h1".into(),
                manager_instance_id: "mgr-1".into(),
                assigned_at: std::time::SystemTime::now(),
                reassigned_from: None,
            })
            .await
            .unwrap();
    }
}

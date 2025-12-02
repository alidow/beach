use async_trait::async_trait;
use manager_sdk::assignment_store::{
    AssignmentStore, AssignmentStoreError, ManagerAssignmentRecord, ManagerInstanceRecord,
};
use sea_query::{Expr, Iden, OnConflict, Order, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::{PgPool, Row};
use std::time::{Duration, SystemTime};

pub struct PostgresAssignmentStore {
    pool: PgPool,
}

impl PostgresAssignmentStore {
    /// Ensure migrations and return a store using SQLx primitives.
    pub async fn ensure_migrations(url: &str) -> Result<(), AssignmentStoreError> {
        let pool = PgPool::connect(url)
            .await
            .map_err(|e| AssignmentStoreError::Store(e.to_string()))?;
        run_migrations(&pool).await
    }

    pub async fn connect(url: &str) -> Result<Self, AssignmentStoreError> {
        let pool = PgPool::connect(url)
            .await
            .map_err(|e| AssignmentStoreError::Store(e.to_string()))?;
        run_migrations(&pool).await?;
        Ok(Self { pool })
    }
}

#[async_trait]
impl AssignmentStore for PostgresAssignmentStore {
    async fn upsert_instance(
        &self,
        instance: ManagerInstanceRecord,
    ) -> Result<(), AssignmentStoreError> {
        let (sql, values) = Query::insert()
            .into_table(ManagerInstances::Table)
            .columns([
                ManagerInstances::Id,
                ManagerInstances::Capacity,
                ManagerInstances::Load,
                ManagerInstances::HeartbeatAt,
            ])
            .values_panic([
                instance.id.clone().into(),
                (instance.capacity as i32).into(),
                (instance.load as i32).into(),
                to_chrono(instance.heartbeat_at).into(),
            ])
            .on_conflict(
                OnConflict::column(ManagerInstances::Id)
                    .update_columns([
                        ManagerInstances::Capacity,
                        ManagerInstances::Load,
                        ManagerInstances::HeartbeatAt,
                    ])
                    .to_owned(),
            )
            .build_sqlx(PostgresQueryBuilder);
        sqlx::query_with(&sql, values)
            .execute(&self.pool)
            .await
            .map_err(|e| AssignmentStoreError::Store(e.to_string()))?;
        Ok(())
    }

    async fn list_instances(&self) -> Result<Vec<ManagerInstanceRecord>, AssignmentStoreError> {
        let (sql, values) = Query::select()
            .columns([
                ManagerInstances::Id,
                ManagerInstances::Capacity,
                ManagerInstances::Load,
                ManagerInstances::HeartbeatAt,
            ])
            .from(ManagerInstances::Table)
            .order_by(ManagerInstances::Id, Order::Asc)
            .build_sqlx(PostgresQueryBuilder);
        let rows = sqlx::query_with(&sql, values)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AssignmentStoreError::Store(e.to_string()))?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let id: Option<String> = r.get("id");
                let capacity: Option<i32> = r.get("capacity");
                let load: Option<i32> = r.get("load");
                let heartbeat_at: Option<chrono::DateTime<chrono::Utc>> = r.get("heartbeat_at");
                match (id, capacity, load, heartbeat_at) {
                    (Some(id), Some(capacity), Some(load), Some(heartbeat_at)) => {
                        Some(ManagerInstanceRecord {
                            id,
                            capacity: capacity as u32,
                            load: load as u32,
                            heartbeat_at: from_chrono(heartbeat_at),
                        })
                    }
                    _ => None,
                }
            })
            .collect())
    }

    async fn live_instances_since(
        &self,
        cutoff: SystemTime,
    ) -> Result<Vec<ManagerInstanceRecord>, AssignmentStoreError> {
        let (sql, values) = Query::select()
            .columns([
                ManagerInstances::Id,
                ManagerInstances::Capacity,
                ManagerInstances::Load,
                ManagerInstances::HeartbeatAt,
            ])
            .from(ManagerInstances::Table)
            .and_where(Expr::col(ManagerInstances::HeartbeatAt).gte(to_chrono(cutoff)))
            .order_by(ManagerInstances::Id, Order::Asc)
            .build_sqlx(PostgresQueryBuilder);
        let rows = sqlx::query_with(&sql, values)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AssignmentStoreError::Store(e.to_string()))?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let id: Option<String> = r.get("id");
                let capacity: Option<i32> = r.get("capacity");
                let load: Option<i32> = r.get("load");
                let heartbeat_at: Option<chrono::DateTime<chrono::Utc>> = r.get("heartbeat_at");
                match (id, capacity, load, heartbeat_at) {
                    (Some(id), Some(capacity), Some(load), Some(heartbeat_at)) => {
                        Some(ManagerInstanceRecord {
                            id,
                            capacity: capacity as u32,
                            load: load as u32,
                            heartbeat_at: from_chrono(heartbeat_at),
                        })
                    }
                    _ => None,
                }
            })
            .collect())
    }

    async fn record_assignment(
        &self,
        record: ManagerAssignmentRecord,
    ) -> Result<(), AssignmentStoreError> {
        let (sql, values) = Query::insert()
            .into_table(ManagerAssignments::Table)
            .columns([
                ManagerAssignments::HostSessionId,
                ManagerAssignments::ManagerInstanceId,
                ManagerAssignments::AssignedAt,
                ManagerAssignments::ReassignedFrom,
            ])
            .values_panic([
                record.host_session_id.clone().into(),
                record.manager_instance_id.clone().into(),
                to_chrono(record.assigned_at).into(),
                record.reassigned_from.clone().into(),
            ])
            .on_conflict(
                OnConflict::column(ManagerAssignments::HostSessionId)
                    .update_columns([
                        ManagerAssignments::ManagerInstanceId,
                        ManagerAssignments::AssignedAt,
                        ManagerAssignments::ReassignedFrom,
                    ])
                    .to_owned(),
            )
            .build_sqlx(PostgresQueryBuilder);
        sqlx::query_with(&sql, values)
            .execute(&self.pool)
            .await
            .map_err(|e| AssignmentStoreError::Store(e.to_string()))?;
        Ok(())
    }

    async fn list_assignments(&self) -> Result<Vec<ManagerAssignmentRecord>, AssignmentStoreError> {
        let (sql, values) = Query::select()
            .columns([
                ManagerAssignments::HostSessionId,
                ManagerAssignments::ManagerInstanceId,
                ManagerAssignments::AssignedAt,
                ManagerAssignments::ReassignedFrom,
            ])
            .from(ManagerAssignments::Table)
            .order_by(ManagerAssignments::HostSessionId, Order::Asc)
            .build_sqlx(PostgresQueryBuilder);
        let rows = sqlx::query_with(&sql, values)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AssignmentStoreError::Store(e.to_string()))?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let host_session_id: Option<String> = r.get("host_session_id");
                let manager_instance_id: Option<String> = r.get("manager_instance_id");
                let assigned_at: Option<chrono::DateTime<chrono::Utc>> = r.get("assigned_at");
                let reassigned_from: Option<String> = r.get("reassigned_from");
                match (host_session_id, manager_instance_id, assigned_at) {
                    (Some(host_session_id), Some(manager_instance_id), Some(assigned_at)) => {
                        Some(ManagerAssignmentRecord {
                            host_session_id,
                            manager_instance_id,
                            assigned_at: from_chrono(assigned_at),
                            reassigned_from,
                        })
                    }
                    _ => None,
                }
            })
            .collect())
    }
}

#[derive(Iden)]
enum ManagerInstances {
    Table,
    Id,
    Capacity,
    Load,
    HeartbeatAt,
}

#[derive(Iden)]
enum ManagerAssignments {
    Table,
    HostSessionId,
    ManagerInstanceId,
    AssignedAt,
    ReassignedFrom,
}

pub fn to_chrono(ts: SystemTime) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::<chrono::Utc>::from(ts)
}

pub fn from_chrono(dt: chrono::DateTime<chrono::Utc>) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(dt.timestamp() as u64)
}

async fn run_migrations(pool: &PgPool) -> Result<(), AssignmentStoreError> {
    sqlx::migrate!()
        .run(pool)
        .await
        .map_err(|e| AssignmentStoreError::Store(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn postgres_round_trip() {
        let url = std::env::var("PG_URL").expect("set PG_URL");
        let store = PostgresAssignmentStore::connect(&url)
            .await
            .expect("connect");
        store
            .upsert_instance(ManagerInstanceRecord {
                id: "pg-mgr".into(),
                capacity: 25,
                load: 5,
                heartbeat_at: SystemTime::now(),
            })
            .await
            .unwrap();
        let instances = store.list_instances().await.unwrap();
        assert!(!instances.is_empty());

        store
            .record_assignment(ManagerAssignmentRecord {
                host_session_id: "pg-host".into(),
                manager_instance_id: "pg-mgr".into(),
                assigned_at: SystemTime::now(),
                reassigned_from: None,
            })
            .await
            .unwrap();
        let assignments = store.list_assignments().await.unwrap();
        assert!(assignments.iter().any(|a| a.host_session_id == "pg-host"));
    }
}

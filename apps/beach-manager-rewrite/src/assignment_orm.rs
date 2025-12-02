use async_trait::async_trait;
use chrono::FixedOffset;
use manager_sdk::assignment_store::{
    AssignmentStore, AssignmentStoreError, ManagerAssignmentRecord, ManagerInstanceRecord,
};
use sea_orm::entity::prelude::*;
use sea_orm::sea_query::OnConflict;
use sea_orm::{Database, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set};

use crate::assignment_postgres::{PostgresAssignmentStore, from_chrono, to_chrono};

pub struct SeaOrmAssignmentStore {
    conn: DatabaseConnection,
}

impl SeaOrmAssignmentStore {
    pub async fn connect(url: &str) -> Result<Self, AssignmentStoreError> {
        // Reuse the SQLx migrations to ensure the schema exists before ORM access.
        PostgresAssignmentStore::ensure_migrations(url).await?;
        let conn = Database::connect(url)
            .await
            .map_err(|e| AssignmentStoreError::Store(e.to_string()))?;
        Ok(Self { conn })
    }
}

#[async_trait]
impl AssignmentStore for SeaOrmAssignmentStore {
    async fn upsert_instance(
        &self,
        instance: ManagerInstanceRecord,
    ) -> Result<(), AssignmentStoreError> {
        let model: manager_instances::ActiveModel = manager_instances::ActiveModel {
            id: Set(instance.id.clone()),
            capacity: Set(instance.capacity as i32),
            load: Set(instance.load as i32),
            heartbeat_at: Set(to_chrono(instance.heartbeat_at).into()),
        };
        manager_instances::Entity::insert(model)
            .on_conflict(
                OnConflict::column(manager_instances::Column::Id)
                    .update_columns([
                        manager_instances::Column::Capacity,
                        manager_instances::Column::Load,
                        manager_instances::Column::HeartbeatAt,
                    ])
                    .to_owned(),
            )
            .exec(&self.conn)
            .await
            .map_err(|e| AssignmentStoreError::Store(e.to_string()))?;
        Ok(())
    }

    async fn list_instances(&self) -> Result<Vec<ManagerInstanceRecord>, AssignmentStoreError> {
        let rows = manager_instances::Entity::find()
            .order_by_asc(manager_instances::Column::Id)
            .all(&self.conn)
            .await
            .map_err(|e| AssignmentStoreError::Store(e.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|r| ManagerInstanceRecord {
                id: r.id,
                capacity: r.capacity as u32,
                load: r.load as u32,
                heartbeat_at: from_chrono(r.heartbeat_at.into()),
            })
            .collect())
    }

    async fn live_instances_since(
        &self,
        cutoff: std::time::SystemTime,
    ) -> Result<Vec<ManagerInstanceRecord>, AssignmentStoreError> {
        let chrono_cutoff = to_chrono(cutoff);
        let cutoff_fixed =
            chrono_cutoff.with_timezone(&FixedOffset::east_opt(0).expect("zero offset"));
        let rows = manager_instances::Entity::find()
            .filter(manager_instances::Column::HeartbeatAt.gte(cutoff_fixed))
            .order_by_asc(manager_instances::Column::Id)
            .all(&self.conn)
            .await
            .map_err(|e| AssignmentStoreError::Store(e.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|r| ManagerInstanceRecord {
                id: r.id,
                capacity: r.capacity as u32,
                load: r.load as u32,
                heartbeat_at: from_chrono(r.heartbeat_at.into()),
            })
            .collect())
    }

    async fn record_assignment(
        &self,
        record: ManagerAssignmentRecord,
    ) -> Result<(), AssignmentStoreError> {
        let model = manager_assignments::ActiveModel {
            host_session_id: Set(record.host_session_id.clone()),
            manager_instance_id: Set(record.manager_instance_id.clone()),
            assigned_at: Set(to_chrono(record.assigned_at).into()),
            reassigned_from: Set(record.reassigned_from.clone()),
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
            .map_err(|e| AssignmentStoreError::Store(e.to_string()))?;
        Ok(())
    }

    async fn list_assignments(&self) -> Result<Vec<ManagerAssignmentRecord>, AssignmentStoreError> {
        let rows = manager_assignments::Entity::find()
            .order_by_asc(manager_assignments::Column::HostSessionId)
            .all(&self.conn)
            .await
            .map_err(|e| AssignmentStoreError::Store(e.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|r| ManagerAssignmentRecord {
                host_session_id: r.host_session_id,
                manager_instance_id: r.manager_instance_id,
                assigned_at: from_chrono(r.assigned_at.into()),
                reassigned_from: r.reassigned_from,
            })
            .collect())
    }
}

pub mod manager_instances {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "manager_instances")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: String,
        pub capacity: i32,
        pub load: i32,
        pub heartbeat_at: DateTimeWithTimeZone,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod manager_assignments {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "manager_assignments")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub host_session_id: String,
        pub manager_instance_id: String,
        pub assigned_at: DateTimeWithTimeZone,
        pub reassigned_from: Option<String>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn seaorm_round_trip() {
        let url = std::env::var("PG_URL").expect("set PG_URL for seaorm test");
        let store = SeaOrmAssignmentStore::connect(&url)
            .await
            .expect("connect seaorm");
        store
            .upsert_instance(ManagerInstanceRecord {
                id: "seaorm-mgr".into(),
                capacity: 10,
                load: 2,
                heartbeat_at: std::time::SystemTime::now(),
            })
            .await
            .unwrap();
        let instances = store.list_instances().await.unwrap();
        assert!(!instances.is_empty());

        store
            .record_assignment(ManagerAssignmentRecord {
                host_session_id: "seaorm-host".into(),
                manager_instance_id: "seaorm-mgr".into(),
                assigned_at: std::time::SystemTime::now(),
                reassigned_from: None,
            })
            .await
            .unwrap();
        let assignments = store.list_assignments().await.unwrap();
        assert!(
            assignments
                .iter()
                .any(|a| a.host_session_id == "seaorm-host")
        );
    }
}

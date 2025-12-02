use async_trait::async_trait;
use manager_sdk::assignment_store::{
    AssignmentStore, AssignmentStoreError, ManagerAssignmentRecord, ManagerInstanceRecord,
};
use redis::aio::ConnectionManager;
use std::time::SystemTime;

pub struct RedisAssignmentStore {
    client: redis::Client,
}

const DEFAULT_TTL_SECS: usize = 15;

impl RedisAssignmentStore {
    pub fn connect(url: &str) -> Result<Self, AssignmentStoreError> {
        let client =
            redis::Client::open(url).map_err(|e| AssignmentStoreError::Store(e.to_string()))?;
        Ok(Self { client })
    }

    async fn conn(&self) -> Result<ConnectionManager, AssignmentStoreError> {
        self.client
            .get_connection_manager()
            .await
            .map_err(|e| AssignmentStoreError::Store(e.to_string()))
    }
}

#[async_trait]
impl AssignmentStore for RedisAssignmentStore {
    async fn upsert_instance(
        &self,
        instance: ManagerInstanceRecord,
    ) -> Result<(), AssignmentStoreError> {
        let mut conn = self.conn().await?;
        let key = format!("mgr:instances:{}", instance.id);
        let _: () = redis::pipe()
            .hset(&key, "capacity", instance.capacity as i64)
            .hset(&key, "load", instance.load as i64)
            .hset(
                &key,
                "heartbeat_at",
                instance
                    .heartbeat_at
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64,
            )
            .expire(&key, DEFAULT_TTL_SECS as i64)
            .query_async(&mut conn)
            .await
            .map_err(|e| AssignmentStoreError::Store(e.to_string()))?;
        let _: () = redis::cmd("SADD")
            .arg("mgr:instance_ids")
            .arg(&instance.id)
            .query_async(&mut conn)
            .await
            .map_err(|e| AssignmentStoreError::Store(e.to_string()))?;
        Ok(())
    }

    async fn list_instances(&self) -> Result<Vec<ManagerInstanceRecord>, AssignmentStoreError> {
        let mut conn = self.conn().await?;
        let ids: Vec<String> = redis::cmd("SMEMBERS")
            .arg("mgr:instance_ids")
            .query_async(&mut conn)
            .await
            .map_err(|e| AssignmentStoreError::Store(e.to_string()))?;
        let mut out = Vec::new();
        for id in ids {
            let key = format!("mgr:instances:{id}");
            let map: redis::Value = redis::cmd("HGETALL")
                .arg(&key)
                .query_async(&mut conn)
                .await
                .map_err(|e| AssignmentStoreError::Store(e.to_string()))?;
            if let redis::Value::Bulk(values) = map {
                let mut capacity = 0;
                let mut load = 0;
                let mut heartbeat_at = 0i64;
                let mut iter = values.chunks(2);
                while let Some(chunk) = iter.next() {
                    if chunk.len() < 2 {
                        continue;
                    }
                    if let (redis::Value::Data(field), redis::Value::Data(val)) =
                        (&chunk[0], &chunk[1])
                    {
                        match field.as_slice() {
                            b"capacity" => {
                                capacity =
                                    String::from_utf8_lossy(val).parse::<i64>().unwrap_or(0) as u32
                            }
                            b"load" => {
                                load =
                                    String::from_utf8_lossy(val).parse::<i64>().unwrap_or(0) as u32
                            }
                            b"heartbeat_at" => {
                                heartbeat_at =
                                    String::from_utf8_lossy(val).parse::<i64>().unwrap_or(0)
                            }
                            _ => {}
                        }
                    }
                }
                out.push(ManagerInstanceRecord {
                    id,
                    capacity,
                    load,
                    heartbeat_at: SystemTime::UNIX_EPOCH
                        + std::time::Duration::from_millis(heartbeat_at.max(0) as u64),
                });
            }
        }
        Ok(out)
    }

    async fn record_assignment(
        &self,
        record: ManagerAssignmentRecord,
    ) -> Result<(), AssignmentStoreError> {
        let mut conn = self.conn().await?;
        let payload = serde_json::to_string(&record)
            .map_err(|e| AssignmentStoreError::Store(e.to_string()))?;
        redis::cmd("RPUSH")
            .arg("mgr:assignment_log")
            .arg(payload)
            .query_async::<_, ()>(&mut conn)
            .await
            .map_err(|e| AssignmentStoreError::Store(e.to_string()))?;
        Ok(())
    }

    async fn list_assignments(&self) -> Result<Vec<ManagerAssignmentRecord>, AssignmentStoreError> {
        let mut conn = self.conn().await?;
        let entries: Vec<String> = redis::cmd("LRANGE")
            .arg("mgr:assignment_log")
            .arg(0)
            .arg(-1)
            .query_async(&mut conn)
            .await
            .map_err(|e| AssignmentStoreError::Store(e.to_string()))?;
        let mut out = Vec::new();
        for entry in entries {
            if let Ok(rec) = serde_json::from_str::<ManagerAssignmentRecord>(&entry) {
                out.push(rec);
            }
        }
        Ok(out)
    }
}

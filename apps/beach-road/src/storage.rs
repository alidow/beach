use anyhow::Result;
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, Client};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub passphrase_hash: String,
    pub created_at: u64,
    pub server_address: Option<String>,
}

impl SessionInfo {
    pub fn new(session_id: String, passphrase_hash: String) -> Self {
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        Self {
            session_id,
            passphrase_hash,
            created_at,
            server_address: None,
        }
    }
}

#[derive(Clone)]
pub struct Storage {
    redis: ConnectionManager,
    ttl_seconds: u64,
}

impl Storage {
    pub async fn new(redis_url: &str, ttl_seconds: u64) -> Result<Self> {
        let client = Client::open(redis_url)?;
        let redis = ConnectionManager::new(client).await?;
        
        Ok(Self {
            redis,
            ttl_seconds,
        })
    }

    pub async fn register_session(&mut self, session: SessionInfo) -> Result<()> {
        let key = format!("session:{}", session.session_id);
        let value = serde_json::to_string(&session)?;
        
        // Set with TTL
        self.redis
            .set_ex(&key, value, self.ttl_seconds)
            .await?;
        
        Ok(())
    }

    pub async fn get_session(&mut self, session_id: &str) -> Result<Option<SessionInfo>> {
        let key = format!("session:{}", session_id);
        let value: Option<String> = self.redis.get(&key).await?;
        
        match value {
            Some(json) => {
                let session = serde_json::from_str(&json)?;
                Ok(Some(session))
            }
            None => Ok(None),
        }
    }

    pub async fn session_exists(&mut self, session_id: &str) -> Result<bool> {
        let key = format!("session:{}", session_id);
        let exists: bool = self.redis.exists(&key).await?;
        Ok(exists)
    }

    pub async fn delete_session(&mut self, session_id: &str) -> Result<()> {
        let key = format!("session:{}", session_id);
        self.redis.del(&key).await?;
        Ok(())
    }

    pub async fn update_session_ttl(&mut self, session_id: &str) -> Result<()> {
        let key = format!("session:{}", session_id);
        self.redis.expire(&key, self.ttl_seconds as i64).await?;
        Ok(())
    }
}
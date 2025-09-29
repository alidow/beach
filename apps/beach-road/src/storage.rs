use anyhow::Result;
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, Client};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::signaling::WebRtcSdpPayload;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub passphrase_hash: String,
    pub created_at: u64,
    pub join_code: String,
    pub server_address: Option<String>,
}

impl SessionInfo {
    pub fn new(session_id: String, passphrase_hash: String, join_code: String) -> Self {
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        Self {
            session_id,
            passphrase_hash,
            created_at,
            join_code,
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

        Ok(Self { redis, ttl_seconds })
    }

    pub async fn register_session(&mut self, session: SessionInfo) -> Result<()> {
        let key = format!("session:{}", session.session_id);
        let value = serde_json::to_string(&session)?;

        // Set with TTL
        self.redis
            .set_ex::<_, _, ()>(&key, value, self.ttl_seconds)
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
        self.redis.del::<_, ()>(&key).await?;
        Ok(())
    }

    pub async fn update_session_ttl(&mut self, session_id: &str) -> Result<()> {
        let key = format!("session:{}", session_id);
        self.redis
            .expire::<_, ()>(&key, self.ttl_seconds as i64)
            .await?;
        Ok(())
    }

    pub async fn push_webrtc_offer(
        &mut self,
        session_id: &str,
        payload: &WebRtcSdpPayload,
    ) -> Result<()> {
        let payload_key = offer_payload_key(session_id, &payload.handshake_id);
        let serialized = serde_json::to_string(payload)?;
        self.redis
            .set_ex::<_, _, ()>(&payload_key, serialized, self.ttl_seconds)
            .await?;

        let queue_key = offer_queue_key(session_id, &payload.to_peer);
        self.redis
            .rpush::<_, _, ()>(&queue_key, payload.handshake_id.clone())
            .await?;
        self.redis
            .expire::<_, ()>(&queue_key, self.ttl_seconds as i64)
            .await?;

        Ok(())
    }

    pub async fn pop_webrtc_offer_for_peer(
        &mut self,
        session_id: &str,
        peer_id: &str,
    ) -> Result<Option<WebRtcSdpPayload>> {
        let queue_key = offer_queue_key(session_id, peer_id);

        loop {
            let handshake_id: Option<String> = self.redis.lpop(&queue_key, None).await?;
            let Some(handshake_id) = handshake_id else {
                return Ok(None);
            };

            let payload_key = offer_payload_key(session_id, &handshake_id);
            let serialized: Option<String> = self.redis.get(&payload_key).await?;
            match serialized {
                Some(json) => {
                    let payload: WebRtcSdpPayload = serde_json::from_str(&json)?;
                    if payload.to_peer != peer_id {
                        self.redis.del::<_, ()>(&payload_key).await?;
                        continue;
                    }
                    return Ok(Some(payload));
                }
                None => continue,
            }
        }
    }

    pub async fn remove_offer_from_queue(
        &mut self,
        session_id: &str,
        peer_id: &str,
        handshake_id: &str,
    ) -> Result<()> {
        let queue_key = offer_queue_key(session_id, peer_id);
        self.redis
            .lrem::<_, _, ()>(&queue_key, 0, handshake_id)
            .await?;
        Ok(())
    }

    pub async fn clear_webrtc_offer_payload(
        &mut self,
        session_id: &str,
        handshake_id: &str,
    ) -> Result<()> {
        let key = offer_payload_key(session_id, handshake_id);
        self.redis.del::<_, ()>(&key).await?;
        Ok(())
    }

    pub async fn store_webrtc_answer(
        &mut self,
        session_id: &str,
        payload: &WebRtcSdpPayload,
    ) -> Result<()> {
        let key = answer_payload_key(session_id, &payload.handshake_id);
        let serialized = serde_json::to_string(payload)?;
        self.redis
            .set_ex::<_, _, ()>(&key, serialized, self.ttl_seconds)
            .await?;
        Ok(())
    }

    pub async fn take_webrtc_answer(
        &mut self,
        session_id: &str,
        handshake_id: &str,
    ) -> Result<Option<WebRtcSdpPayload>> {
        let key = answer_payload_key(session_id, handshake_id);
        let serialized: Option<String> = self.redis.get(&key).await?;
        match serialized {
            Some(json) => {
                self.redis.del::<_, ()>(&key).await?;
                let payload = serde_json::from_str(&json)?;
                Ok(Some(payload))
            }
            None => Ok(None),
        }
    }
}

fn offer_payload_key(session_id: &str, handshake_id: &str) -> String {
    format!("session:{}:webrtc:offer:{}", session_id, handshake_id)
}

fn offer_queue_key(session_id: &str, peer_id: &str) -> String {
    format!("session:{}:webrtc:offers:{}", session_id, peer_id)
}

fn answer_payload_key(session_id: &str, handshake_id: &str) -> String {
    format!("session:{}:webrtc:answer:{}", session_id, handshake_id)
}

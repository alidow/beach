use anyhow::Result;
use beach_lifeguard_core::{GuardrailCounters, GuardrailSnapshot};
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, Client};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use time::OffsetDateTime;

use crate::signaling::WebRtcSdpPayload;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub passphrase_hash: String,
    pub created_at: u64,
    pub join_code: String,
    pub server_address: Option<String>,
    #[serde(default)]
    pub owner_account_id: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub location_hint: Option<String>,
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
            owner_account_id: None,
            kind: None,
            title: None,
            location_hint: None,
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

    pub async fn register_session(&self, session: SessionInfo) -> Result<()> {
        let mut conn = self.redis.clone();
        let key = format!("session:{}", session.session_id);
        let value = serde_json::to_string(&session)?;

        // Set with TTL
        conn.set_ex::<_, _, ()>(&key, value, self.ttl_seconds)
            .await?;

        Ok(())
    }

    pub async fn get_session(&self, session_id: &str) -> Result<Option<SessionInfo>> {
        let mut conn = self.redis.clone();
        let key = format!("session:{}", session_id);
        let value: Option<String> = conn.get(&key).await?;

        match value {
            Some(json) => {
                let session = serde_json::from_str(&json)?;
                Ok(Some(session))
            }
            None => Ok(None),
        }
    }

    pub async fn list_sessions(&self) -> Result<Vec<SessionInfo>> {
        let mut conn = self.redis.clone();
        let mut cursor: u64 = 0;
        let mut results = Vec::new();
        loop {
            let (next_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .cursor_arg(cursor)
                .arg("MATCH")
                .arg("session:*")
                .arg("COUNT")
                .arg(100u32)
                .query_async(&mut conn)
                .await?;
            cursor = next_cursor;
            if !keys.is_empty() {
                let values: Vec<Option<String>> =
                    redis::cmd("MGET").arg(keys).query_async(&mut conn).await?;
                for v in values.into_iter().flatten() {
                    if let Ok(s) = serde_json::from_str::<SessionInfo>(&v) {
                        results.push(s);
                    }
                }
            }
            if cursor == 0 {
                break;
            }
        }
        Ok(results)
    }

    pub async fn session_exists(&self, session_id: &str) -> Result<bool> {
        let mut conn = self.redis.clone();
        let key = format!("session:{}", session_id);
        let exists: bool = conn.exists(&key).await?;
        Ok(exists)
    }

    pub async fn delete_session(&self, session_id: &str) -> Result<()> {
        let mut conn = self.redis.clone();
        let key = format!("session:{}", session_id);
        conn.del::<_, ()>(&key).await?;
        Ok(())
    }

    pub async fn update_session_ttl(&self, session_id: &str) -> Result<()> {
        let mut conn = self.redis.clone();
        let key = format!("session:{}", session_id);
        conn.expire::<_, ()>(&key, self.ttl_seconds as i64).await?;
        Ok(())
    }

    pub async fn push_webrtc_offer(
        &self,
        session_id: &str,
        payload: &WebRtcSdpPayload,
    ) -> Result<()> {
        let mut conn = self.redis.clone();
        let payload_key = offer_payload_key(session_id, &payload.handshake_id);
        let serialized = serde_json::to_string(payload)?;
        conn.set_ex::<_, _, ()>(&payload_key, serialized, self.ttl_seconds)
            .await?;

        let queue_key = offer_queue_key(session_id, &payload.to_peer);
        conn.rpush::<_, _, ()>(&queue_key, payload.handshake_id.clone())
            .await?;
        conn.expire::<_, ()>(&queue_key, self.ttl_seconds as i64)
            .await?;

        Ok(())
    }

    pub async fn pop_webrtc_offer_for_peer(
        &self,
        session_id: &str,
        peer_id: &str,
    ) -> Result<Option<WebRtcSdpPayload>> {
        let mut conn = self.redis.clone();
        let queue_key = offer_queue_key(session_id, peer_id);

        loop {
            let handshake_id: Option<String> = conn.lpop(&queue_key, None).await?;
            let Some(handshake_id) = handshake_id else {
                return Ok(None);
            };

            let payload_key = offer_payload_key(session_id, &handshake_id);
            let serialized: Option<String> = conn.get(&payload_key).await?;
            match serialized {
                Some(json) => {
                    let payload: WebRtcSdpPayload = serde_json::from_str(&json)?;
                    if payload.to_peer != peer_id {
                        conn.del::<_, ()>(&payload_key).await?;
                        continue;
                    }
                    return Ok(Some(payload));
                }
                None => continue,
            }
        }
    }

    pub async fn remove_offer_from_queue(
        &self,
        session_id: &str,
        peer_id: &str,
        handshake_id: &str,
    ) -> Result<()> {
        let mut conn = self.redis.clone();
        let queue_key = offer_queue_key(session_id, peer_id);
        conn.lrem::<_, _, ()>(&queue_key, 0, handshake_id).await?;
        Ok(())
    }

    pub async fn clear_webrtc_offer_payload(
        &self,
        session_id: &str,
        handshake_id: &str,
    ) -> Result<()> {
        let mut conn = self.redis.clone();
        let key = offer_payload_key(session_id, handshake_id);
        conn.del::<_, ()>(&key).await?;
        Ok(())
    }

    pub async fn store_webrtc_answer(
        &self,
        session_id: &str,
        payload: &WebRtcSdpPayload,
    ) -> Result<()> {
        let mut conn = self.redis.clone();
        let key = answer_payload_key(session_id, &payload.handshake_id);
        let serialized = serde_json::to_string(payload)?;
        conn.set_ex::<_, _, ()>(&key, serialized, self.ttl_seconds)
            .await?;
        Ok(())
    }

    pub async fn take_webrtc_answer(
        &self,
        session_id: &str,
        handshake_id: &str,
    ) -> Result<Option<WebRtcSdpPayload>> {
        let mut conn = self.redis.clone();
        let key = answer_payload_key(session_id, handshake_id);
        let serialized: Option<String> = conn.get(&key).await?;
        match serialized {
            Some(json) => {
                conn.del::<_, ()>(&key).await?;
                let payload = serde_json::from_str(&json)?;
                Ok(Some(payload))
            }
            None => Ok(None),
        }
    }

    pub async fn track_fallback_activation(
        &self,
        cohort_id: &str,
        total_sessions_hint: Option<u64>,
    ) -> Result<GuardrailSnapshot> {
        let mut conn = self.redis.clone();
        let now = OffsetDateTime::now_utc();
        let bucket = guardrail_bucket(now);

        let fallback_key = guardrail_fallback_key(cohort_id, &bucket);
        let total_key = guardrail_total_key(cohort_id, &bucket);
        let ttl_seconds = 90 * 60; // 90 minutes to cover an hour bucket plus buffer

        let fallback_sessions: u64 = {
            let count: u64 = conn.incr(&fallback_key, 1).await?;
            if count == 1 {
                let _: () = conn.expire(&fallback_key, ttl_seconds).await?;
            }
            count
        };

        let stored_total = if let Some(total_hint) = total_sessions_hint {
            let _: () = conn.set(&total_key, total_hint).await?;
            let _: () = conn.expire(&total_key, ttl_seconds).await?;
            total_hint
        } else {
            let existing: Option<u64> = conn.get(&total_key).await?;
            existing.unwrap_or(fallback_sessions)
        };

        let counters = GuardrailCounters {
            total_sessions: stored_total.max(fallback_sessions),
            fallback_sessions,
        };

        Ok(GuardrailSnapshot::new(now, counters))
    }
}

fn guardrail_bucket(now: OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}-{:02}",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour()
    )
}

fn guardrail_fallback_key(cohort_id: &str, bucket: &str) -> String {
    format!("fallback:cohort:{}:{}:fallback", cohort_id, bucket)
}

fn guardrail_total_key(cohort_id: &str, bucket: &str) -> String {
    format!("fallback:cohort:{}:{}:total", cohort_id, bucket)
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

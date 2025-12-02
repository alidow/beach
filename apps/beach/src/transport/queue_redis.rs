use std::time::Duration;

use async_trait::async_trait;
use beach_buggy::{ActionAck, ActionCommand, StateDiff};
use redis::aio::ConnectionManager;
use redis::streams::StreamReadOptions;
use redis::{AsyncCommands, RedisResult};

use super::queue::{ControllerQueue, ControllerQueueConsumer, QueueError};

const STREAM_ACTION: &str = "beach:queue:actions";
const STREAM_ACK: &str = "beach:queue:acks";
const STREAM_STATE: &str = "beach:queue:states";
const GROUP: &str = "beach-manager";

/// Redis Streams-backed queue for action/ack/state messages.
pub struct RedisQueue {
    client: redis::Client,
}

impl RedisQueue {
    pub fn new(url: &str) -> RedisResult<Self> {
        let client = redis::Client::open(url)?;
        Ok(Self { client })
    }

    async fn conn(&self) -> RedisResult<ConnectionManager> {
        let conn = self.client.get_connection_manager().await?;
        Ok(conn)
    }

    pub async fn init(&self) -> RedisResult<()> {
        let mut conn = self.conn().await?;
        let _: () = redis::cmd("XGROUP")
            .arg("CREATE")
            .arg(STREAM_ACTION)
            .arg(GROUP)
            .arg("0")
            .arg("MKSTREAM")
            .query_async(&mut conn)
            .await
            .or_else(ignore_exists)?;
        let _: () = redis::cmd("XGROUP")
            .arg("CREATE")
            .arg(STREAM_ACK)
            .arg(GROUP)
            .arg("0")
            .arg("MKSTREAM")
            .query_async(&mut conn)
            .await
            .or_else(ignore_exists)?;
        let _: () = redis::cmd("XGROUP")
            .arg("CREATE")
            .arg(STREAM_STATE)
            .arg(GROUP)
            .arg("0")
            .arg("MKSTREAM")
            .query_async(&mut conn)
            .await
            .or_else(ignore_exists)?;
        Ok(())
    }

    pub async fn enqueue_action(&self, action: ActionCommand) -> RedisResult<()> {
        let mut conn = self.conn().await?;
        let payload = serde_json::to_string(&action).map_err(to_redis_err)?;
        let _: () = conn
            .xadd(STREAM_ACTION, "*", &[("payload", payload.as_str())])
            .await?;
        Ok(())
    }

    pub async fn enqueue_ack(&self, ack: ActionAck) -> RedisResult<()> {
        let mut conn = self.conn().await?;
        let payload = serde_json::to_string(&ack).map_err(to_redis_err)?;
        let _: () = conn
            .xadd(STREAM_ACK, "*", &[("payload", payload.as_str())])
            .await?;
        Ok(())
    }

    pub async fn enqueue_state(&self, state: StateDiff) -> RedisResult<()> {
        let mut conn = self.conn().await?;
        let payload = serde_json::to_string(&state).map_err(to_redis_err)?;
        let _: () = conn
            .xadd(STREAM_STATE, "*", &[("payload", payload.as_str())])
            .await?;
        Ok(())
    }

    pub async fn read_batch<T: for<'de> serde::Deserialize<'de>>(
        &self,
        stream: &str,
        consumer: &str,
        count: usize,
        block: Duration,
    ) -> RedisResult<Vec<(String, T)>> {
        let mut conn = self.conn().await?;
        let mut opts = StreamReadOptions::default();
        opts = opts
            .group(GROUP, consumer)
            .count(count)
            .block(block.as_millis() as usize);

        let entries: redis::streams::StreamReadReply =
            conn.xread_options(&[stream], &[">"], &opts).await?;
        let mut out = Vec::new();
        for stream_reply in entries.keys {
            for msg in stream_reply.ids {
                if let Some(payload) = msg.map.get("payload") {
                    if let Ok(payload_str) = redis::from_redis_value::<String>(payload) {
                        if let Ok(item) = serde_json::from_str::<T>(&payload_str) {
                            out.push((msg.id.to_string(), item));
                        }
                    }
                }
            }
        }
        // Acknowledge claimed messages
        for (id, _) in &out {
            let _: () = redis::cmd("XACK")
                .arg(stream)
                .arg(GROUP)
                .arg(id)
                .query_async(&mut conn)
                .await?;
        }
        // Add a small retention to prevent unbounded growth
        let _: () = redis::cmd("XTRIM")
            .arg(stream)
            .arg("MAXLEN")
            .arg("~")
            .arg(10_000)
            .query_async(&mut conn)
            .await?;
        Ok(out)
    }
}

fn ignore_exists(err: redis::RedisError) -> RedisResult<()> {
    if err.code() == Some("BUSYGROUP") {
        Ok(())
    } else {
        Err(err)
    }
}

fn to_redis_err(err: impl std::error::Error) -> redis::RedisError {
    redis::RedisError::from((redis::ErrorKind::TypeError, "serialize", err.to_string()))
}

impl From<redis::RedisError> for QueueError {
    fn from(err: redis::RedisError) -> Self {
        QueueError::Backend(err.to_string())
    }
}

#[async_trait]
impl ControllerQueue for RedisQueue {
    async fn enqueue_action(&self, action: ActionCommand) -> Result<(), QueueError> {
        RedisQueue::enqueue_action(self, action)
            .await
            .map_err(QueueError::from)
    }

    async fn enqueue_ack(&self, ack: ActionAck) -> Result<(), QueueError> {
        RedisQueue::enqueue_ack(self, ack)
            .await
            .map_err(QueueError::from)
    }

    async fn enqueue_state(&self, state: StateDiff) -> Result<(), QueueError> {
        RedisQueue::enqueue_state(self, state)
            .await
            .map_err(QueueError::from)
    }
}

#[async_trait]
impl ControllerQueueConsumer for RedisQueue {
    async fn drain_actions(&self, max: usize) -> Result<Vec<ActionCommand>, QueueError> {
        self.read_batch(
            STREAM_ACTION,
            "consumer-actions",
            max,
            Duration::from_millis(10),
        )
        .await
        .map(|items| items.into_iter().map(|(_, v)| v).collect())
        .map_err(QueueError::from)
    }

    async fn drain_acks(&self, max: usize) -> Result<Vec<ActionAck>, QueueError> {
        self.read_batch(STREAM_ACK, "consumer-acks", max, Duration::from_millis(10))
            .await
            .map(|items| items.into_iter().map(|(_, v)| v).collect())
            .map_err(QueueError::from)
    }

    async fn drain_states(&self, max: usize) -> Result<Vec<StateDiff>, QueueError> {
        self.read_batch(
            STREAM_STATE,
            "consumer-states",
            max,
            Duration::from_millis(10),
        )
        .await
        .map(|items| items.into_iter().map(|(_, v)| v).collect())
        .map_err(QueueError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beach_buggy::AckStatus;

    // Integration test gated behind the `redis` feature and requires REDIS_URL env.
    #[tokio::test]
    #[ignore]
    async fn redis_queue_roundtrip() {
        let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1/".into());
        let queue = RedisQueue::new(&url).expect("redis client");
        queue.init().await.expect("init");

        let action = ActionCommand {
            id: "r1".into(),
            action_type: "terminal_write".into(),
            payload: serde_json::json!({"bytes": "hi"}),
            expires_at: None,
        };
        queue.enqueue_action(action).await.expect("enqueue action");

        let ack = ActionAck {
            id: "r1".into(),
            status: AckStatus::Ok,
            applied_at: std::time::SystemTime::now(),
            latency_ms: None,
            error_code: None,
            error_message: None,
        };
        queue.enqueue_ack(ack).await.expect("enqueue ack");

        let state = StateDiff {
            sequence: 1,
            emitted_at: std::time::SystemTime::now(),
            payload: serde_json::json!({"seq": 1}),
        };
        queue.enqueue_state(state).await.expect("enqueue state");

        let actions = queue
            .read_batch::<ActionCommand>(
                STREAM_ACTION,
                "consumer-1",
                10,
                Duration::from_millis(500),
            )
            .await
            .expect("read actions");
        assert!(actions.iter().any(|(_, a)| a.id == "r1"));

        let acks = queue
            .read_batch::<ActionAck>(STREAM_ACK, "consumer-1", 10, Duration::from_millis(500))
            .await
            .expect("read acks");
        assert!(acks.iter().any(|(_, a)| a.id == "r1"));

        let states = queue
            .read_batch::<StateDiff>(STREAM_STATE, "consumer-1", 10, Duration::from_millis(500))
            .await
            .expect("read states");
        assert!(states.iter().any(|(_, s)| s.sequence == 1));
    }
}

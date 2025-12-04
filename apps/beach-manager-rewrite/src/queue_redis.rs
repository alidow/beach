use async_trait::async_trait;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;

use crate::queue::{ActionAck, ActionCommand, ControllerQueue, StateDiff};

const LIST_ACTIONS: &str = "beach:rewrite:actions";
const LIST_ACKS: &str = "beach:rewrite:acks";
const LIST_STATES: &str = "beach:rewrite:states";
const MAX_LIST_LEN: isize = 10_000; // basic backpressure cap per list

pub struct RedisQueue {
    client: redis::Client,
}

impl RedisQueue {
    pub fn connect(url: &str) -> redis::RedisResult<Self> {
        let client = redis::Client::open(url)?;
        Ok(Self { client })
    }

    async fn conn(&self) -> redis::RedisResult<ConnectionManager> {
        self.client.get_connection_manager().await
    }

    async fn push_json<T: serde::Serialize>(
        &self,
        list: &str,
        value: &T,
    ) -> redis::RedisResult<()> {
        let mut conn = self.conn().await?;
        let payload = serde_json::to_string(value).map_err(to_redis_err)?;
        let _: () = conn.rpush(list, payload).await?;
        // Trim to max length to prevent unbounded growth.
        let _: () = redis::cmd("LTRIM")
            .arg(list)
            .arg(-MAX_LIST_LEN)
            .arg(-1)
            .query_async(&mut conn)
            .await?;
        Ok(())
    }

    async fn pop_json<T: for<'de> serde::Deserialize<'de>>(
        &self,
        list: &str,
        max: usize,
    ) -> redis::RedisResult<Vec<T>> {
        let mut conn = self.conn().await?;
        let mut out = Vec::new();
        for _ in 0..max {
            let item: Option<String> = conn.lpop(list, None).await?;
            if let Some(payload) = item {
                if let Ok(value) = serde_json::from_str::<T>(&payload) {
                    out.push(value);
                }
            } else {
                break;
            }
        }
        Ok(out)
    }
}

fn to_redis_err(err: impl std::error::Error) -> redis::RedisError {
    redis::RedisError::from((redis::ErrorKind::TypeError, "serialize", err.to_string()))
}

#[async_trait]
impl ControllerQueue for RedisQueue {
    async fn enqueue_action(&self, action: ActionCommand) {
        let _ = self.push_json(LIST_ACTIONS, &action).await;
        crate::metrics::QUEUE_ENQUEUED
            .with_label_values(&["action"])
            .inc();
    }

    async fn enqueue_ack(&self, ack: ActionAck) {
        let _ = self.push_json(LIST_ACKS, &ack).await;
        crate::metrics::QUEUE_ENQUEUED
            .with_label_values(&["ack"])
            .inc();
    }

    async fn enqueue_state(&self, state: StateDiff) {
        let _ = self.push_json(LIST_STATES, &state).await;
        crate::metrics::QUEUE_ENQUEUED
            .with_label_values(&["state"])
            .inc();
    }

    async fn drain_actions(&self, max: usize) -> Vec<ActionCommand> {
        self.pop_json(LIST_ACTIONS, max).await.unwrap_or_default()
    }

    async fn drain_acks(&self, max: usize) -> Vec<ActionAck> {
        self.pop_json(LIST_ACKS, max).await.unwrap_or_default()
    }

    async fn drain_states(&self, max: usize) -> Vec<StateDiff> {
        self.pop_json(LIST_STATES, max).await.unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn redis_queue_roundtrip() {
        let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1/".into());
        let queue = RedisQueue::connect(&url).expect("redis");
        queue
            .enqueue_action(ActionCommand {
                id: "r1".into(),
                action_type: "write".into(),
                payload: serde_json::json!({"bytes": "hi"}),
            })
            .await;

        let actions = queue.drain_actions(10).await;
        assert!(actions.iter().any(|a| a.id == "r1"));
    }
}

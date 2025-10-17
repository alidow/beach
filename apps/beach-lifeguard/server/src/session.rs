use std::{
    borrow::Cow,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::extract::ws::{close_code, CloseFrame, Message};
use dashmap::{mapref::entry::Entry, DashMap};
use metrics::counter;
use slab::Slab;
use tokio::{
    sync::{mpsc, Mutex},
    task::JoinHandle,
};
use uuid::Uuid;

use beach_lifeguard_client::CompressionStrategy;

const DEFAULT_CHANNEL_DEPTH: usize = 64;

#[derive(Clone)]
pub struct SessionRegistry {
    inner: Arc<SessionRegistryInner>,
}

struct SessionRegistryInner {
    sessions: DashMap<Uuid, Arc<SessionState>>,
    config: SessionConfig,
    total_sessions: parking_lot::RwLock<usize>,
}

#[derive(Clone)]
pub struct SessionConfig {
    pub per_connection_buffer: usize,
    pub idle_timeout: Duration,
    pub recycle_interval: Duration,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            per_connection_buffer: DEFAULT_CHANNEL_DEPTH,
            idle_timeout: Duration::from_secs(120),
            recycle_interval: Duration::from_secs(30),
        }
    }
}

impl SessionRegistry {
    pub fn new(config: SessionConfig) -> Self {
        Self {
            inner: Arc::new(SessionRegistryInner {
                sessions: DashMap::new(),
                config,
                total_sessions: parking_lot::RwLock::new(0),
            }),
        }
    }

    pub async fn register(
        &self,
        session_id: Uuid,
        connection_id: Uuid,
        compression: CompressionStrategy,
    ) -> SessionRegistration {
        let (tx, rx) = mpsc::channel(self.inner.config.per_connection_buffer);
        let now = now_millis();

        let mut new_session = false;
        let state = match self.inner.sessions.entry(session_id) {
            Entry::Occupied(entry) => Arc::clone(entry.get()),
            Entry::Vacant(entry) => {
                let state = Arc::new(SessionState::new(session_id));
                entry.insert(Arc::clone(&state));
                new_session = true;
                state
            }
        };

        if new_session {
            *self.inner.total_sessions.write() += 1;
        }

        let active_connections = state
            .add_connection(connection_id, tx, compression, now)
            .await;

        SessionRegistration {
            receiver: rx,
            active_connections,
            total_sessions: *self.inner.total_sessions.read(),
        }
    }

    pub async fn unregister(&self, session_id: Uuid, connection_id: Uuid) -> SessionRemoval {
        let mut active_connections = 0usize;

        if let Some(entry) = self.inner.sessions.get_mut(&session_id) {
            let state = Arc::clone(entry.value());
            drop(entry);

            active_connections = state.remove_connection(connection_id).await;
            if active_connections == 0 {
                if self
                    .inner
                    .sessions
                    .remove_if(&session_id, |_, arc| Arc::ptr_eq(arc, &state))
                    .is_some()
                {
                    *self.inner.total_sessions.write() -= 1;
                }
            }
        }

        SessionRemoval {
            active_connections,
            total_sessions: *self.inner.total_sessions.read(),
        }
    }

    pub async fn broadcast(
        &self,
        session_id: Uuid,
        source_id: Uuid,
        message: Message,
    ) -> BroadcastMetrics {
        if let Some(entry) = self.inner.sessions.get(&session_id) {
            let state = Arc::clone(entry.value());
            drop(entry);
            return state.broadcast(source_id, message).await;
        }

        BroadcastMetrics::default()
    }

    pub async fn force_close_idle(&self, now: u64) -> usize {
        let mut idle_total = 0usize;
        let session_ids: Vec<Uuid> = self
            .inner
            .sessions
            .iter()
            .map(|entry| *entry.key())
            .collect();

        for session_id in session_ids {
            let Some(entry) = self.inner.sessions.get(&session_id) else {
                continue;
            };
            let state = Arc::clone(entry.value());
            drop(entry);

            let idle = state
                .collect_idle(now, self.inner.config.idle_timeout)
                .await;
            for connection_id in idle {
                if let Some(close_msg) = state.prepare_idle_close(connection_id).await {
                    counter!(
                        "beach_lifeguard_idle_pruned_total",
                        1,
                        "session_id" => session_id.to_string()
                    );
                    let _ = close_msg.sender.try_send(Message::Close(Some(CloseFrame {
                        code: close_code::POLICY,
                        reason: Cow::Owned("idle timeout".into()),
                    })));

                    drop(close_msg);
                }

                let removal = self.unregister(session_id, connection_id).await;
                if removal.active_connections == 0 {
                    counter!(
                        "beach_lifeguard_sessions_emptied_total",
                        1,
                        "session_id" => session_id.to_string()
                    );
                }
                idle_total += 1;
            }
        }

        idle_total
    }

    pub async fn snapshot(&self) -> Vec<SessionSnapshot> {
        let session_ids: Vec<Uuid> = self
            .inner
            .sessions
            .iter()
            .map(|entry| *entry.key())
            .collect();

        let mut snapshots = Vec::with_capacity(session_ids.len());
        for id in session_ids {
            if let Some(state_ref) = self.inner.sessions.get(&id) {
                let state = Arc::clone(state_ref.value());
                drop(state_ref);
                let count = state.connection_count().await;
                snapshots.push(SessionSnapshot {
                    session_id: id,
                    connections: count,
                });
            }
        }

        snapshots
    }

    pub fn spawn_recycler(&self) -> JoinHandle<()> {
        let registry = self.clone();
        let mut interval = tokio::time::interval(self.inner.config.recycle_interval);
        tokio::spawn(async move {
            loop {
                interval.tick().await;
                let now = now_millis();
                let _ = registry.force_close_idle(now).await;
            }
        })
    }
}

pub struct SessionRegistration {
    pub receiver: mpsc::Receiver<Message>,
    pub active_connections: usize,
    pub total_sessions: usize,
}

pub struct SessionRemoval {
    pub active_connections: usize,
    pub total_sessions: usize,
}

#[derive(Default)]
pub struct BroadcastMetrics {
    pub delivered: usize,
    pub bytes: usize,
    pub dropped: usize,
    pub closed: usize,
}

pub struct SessionSnapshot {
    pub session_id: Uuid,
    pub connections: usize,
}

struct SessionState {
    session_id: Uuid,
    inner: Mutex<SessionStateInner>,
}

struct SessionStateInner {
    slab: Slab<ConnectionEntry>,
    index_map: std::collections::HashMap<Uuid, usize>,
}

struct ConnectionEntry {
    id: Uuid,
    sender: mpsc::Sender<Message>,
    compression: CompressionStrategy,
    last_activity: AtomicU64,
}

struct IdleClose {
    sender: mpsc::Sender<Message>,
}

impl SessionState {
    fn new(session_id: Uuid) -> Self {
        Self {
            session_id,
            inner: Mutex::new(SessionStateInner {
                slab: Slab::new(),
                index_map: std::collections::HashMap::new(),
            }),
        }
    }

    async fn add_connection(
        &self,
        connection_id: Uuid,
        sender: mpsc::Sender<Message>,
        compression: CompressionStrategy,
        now: u64,
    ) -> usize {
        let mut guard = self.inner.lock().await;
        let entry = ConnectionEntry {
            id: connection_id,
            sender,
            compression,
            last_activity: AtomicU64::new(now),
        };
        let key = guard.slab.insert(entry);
        guard.index_map.insert(connection_id, key);
        guard.slab.len()
    }

    async fn remove_connection(&self, connection_id: Uuid) -> usize {
        let mut guard = self.inner.lock().await;
        if let Some(index) = guard.index_map.remove(&connection_id) {
            guard.slab.remove(index);
        }
        guard.slab.len()
    }

    async fn broadcast(&self, source_id: Uuid, message: Message) -> BroadcastMetrics {
        let mut metrics = BroadcastMetrics::default();
        let now = now_millis();

        let mut pending = Vec::new();
        {
            let mut guard = self.inner.lock().await;
            for (idx, entry) in guard.slab.iter_mut() {
                if entry.id == source_id {
                    entry.last_activity.store(now, Ordering::Relaxed);
                    continue;
                }
                pending.push((idx, entry.id, entry.sender.clone(), entry.compression));
            }
        }

        let bytes = message_len(&message);
        for (idx, connection_id, sender, compression) in pending {
            let outbound = maybe_encode(&message, compression);
            match sender.try_send(outbound) {
                Ok(_) => {
                    metrics.delivered += 1;
                    metrics.bytes += bytes;
                }
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    metrics.dropped += 1;
                    counter!(
                        "beach_lifeguard_flow_control_drops_total",
                        1,
                        "session_id" => self.session_id.to_string(),
                        "connection_id" => connection_id.to_string()
                    );
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    metrics.closed += 1;
                    self.evict_closed(idx, connection_id).await;
                }
            }
        }

        metrics
    }

    async fn evict_closed(&self, index: usize, connection_id: Uuid) {
        let mut guard = self.inner.lock().await;
        guard.slab.remove(index);
        guard.index_map.remove(&connection_id);
    }

    async fn collect_idle(&self, now: u64, timeout: Duration) -> Vec<Uuid> {
        let mut idle = Vec::new();
        let guard = self.inner.lock().await;
        for (_, entry) in guard.slab.iter() {
            let last = entry.last_activity.load(Ordering::Relaxed);
            if now.saturating_sub(last) > timeout.as_millis() as u64 {
                idle.push(entry.id);
            }
        }
        idle
    }

    async fn prepare_idle_close(&self, connection_id: Uuid) -> Option<IdleClose> {
        let guard = self.inner.lock().await;
        if let Some(index) = guard.index_map.get(&connection_id).copied() {
            if let Some(entry) = guard.slab.get(index) {
                return Some(IdleClose {
                    sender: entry.sender.clone(),
                });
            }
        }
        None
    }

    async fn connection_count(&self) -> usize {
        let guard = self.inner.lock().await;
        guard.slab.len()
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn maybe_encode(message: &Message, compression: CompressionStrategy) -> Message {
    match compression {
        CompressionStrategy::None => message.clone(),
        CompressionStrategy::Brotli => match message {
            Message::Binary(bytes) => Message::Binary(bytes.clone()),
            Message::Text(text) => Message::Binary(text.as_bytes().to_vec()),
            other => other.clone(),
        },
    }
}

fn message_len(message: &Message) -> usize {
    match message {
        Message::Text(text) => text.as_bytes().len(),
        Message::Binary(bytes) => bytes.len(),
        _ => 0,
    }
}

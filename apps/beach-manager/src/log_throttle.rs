use once_cell::sync::Lazy;
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueueLogKind {
    Request,
    Validate,
    Overflow,
}

impl QueueLogKind {
    fn interval(self) -> Duration {
        match self {
            QueueLogKind::Request => Duration::from_secs(10),
            QueueLogKind::Validate => Duration::from_secs(10),
            QueueLogKind::Overflow => Duration::from_secs(60),
        }
    }
}

#[derive(Eq, PartialEq, Hash)]
struct LogKey {
    kind: QueueLogKind,
    session_id: String,
}

static QUEUE_LOG_MEMORY: Lazy<Mutex<HashMap<LogKey, Instant>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub fn should_log_queue_event(kind: QueueLogKind, session_id: &str) -> bool {
    let mut guard = QUEUE_LOG_MEMORY
        .lock()
        .expect("queue log throttle mutex poisoned");
    let key = LogKey {
        kind,
        session_id: session_id.to_string(),
    };
    let now = Instant::now();
    if let Some(last) = guard.get(&key) {
        if now.duration_since(*last) < kind.interval() {
            return false;
        }
    }
    guard.insert(key, now);
    true
}

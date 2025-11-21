#![allow(dead_code)]

use std::sync::Arc;

use beach_buggy::ActionCommand;

/// Minimal stub of the legacy fast-path module. The unified transport now carries
/// controller traffic via extension frames, so this file only retains the helpers
/// still referenced elsewhere.
#[derive(Clone, Default)]
pub struct FastPathRegistry;

impl FastPathRegistry {
    pub fn new() -> Self {
        Self
    }

    pub async fn insert(&self, _session_id: String, _fps: Arc<FastPathSession>) {}

    pub async fn get(&self, _session_id: &str) -> Option<Arc<FastPathSession>> {
        None
    }

    pub async fn remove(&self, _session_id: &str) -> Option<Arc<FastPathSession>> {
        None
    }
}

#[derive(Clone)]
pub struct FastPathSession;

impl FastPathSession {
    pub fn instance_id(&self) -> u64 {
        0
    }

    pub fn spawn_receivers(self: &Arc<Self>, _state: crate::state::AppState) {}
}

fn terminal_write_bytes(action: &ActionCommand) -> Result<&str, String> {
    if action.action_type.as_str() != "terminal_write" {
        return Err(format!("unsupported action type {}", action.action_type));
    }
    action
        .payload
        .get("bytes")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "terminal_write payload missing bytes".to_string())
}

pub fn fast_path_action_bytes(action: &ActionCommand) -> Result<Vec<u8>, String> {
    let bytes = terminal_write_bytes(action)?;
    Ok(bytes.as_bytes().to_vec())
}

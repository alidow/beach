use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::server::terminal::host::ControllerBridge;
use crate::server::terminal::{PtyProcess, PtyWriter};
use crate::sync::terminal::TerminalSync;

#[derive(Clone)]
pub struct TerminalSession {
    pub session_id: String,
    pub sync: Arc<TerminalSync>,
    pub writer: PtyWriter,
    pub process: Arc<PtyProcess>,
    pub controller_bridge: Option<Arc<ControllerBridge>>,
}

impl TerminalSession {
    pub fn new(
        session_id: impl Into<String>,
        sync: Arc<TerminalSync>,
        writer: PtyWriter,
        process: Arc<PtyProcess>,
        controller_bridge: Option<Arc<ControllerBridge>>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            sync,
            writer,
            process,
            controller_bridge,
        }
    }
}

#[derive(Clone, Default)]
pub struct SessionRegistry {
    inner: Arc<RwLock<HashMap<String, SessionEntry>>>,
}

#[derive(Clone)]
struct SessionEntry {
    terminal: Option<Arc<TerminalSession>>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn register_terminal(&self, session: TerminalSession) -> RegistryGuard {
        let mut map = self.inner.write().unwrap();
        let entry = map
            .entry(session.session_id.clone())
            .or_insert_with(|| SessionEntry { terminal: None });
        entry.terminal = Some(Arc::new(session));
        RegistryGuard {
            registry: self.clone(),
            session_id: entry
                .terminal
                .as_ref()
                .map(|surface| surface.session_id.clone())
                .expect("terminal session must exist"),
        }
    }

    pub fn get_terminal(&self, session_id: &str) -> Option<Arc<TerminalSession>> {
        let map = self.inner.read().unwrap();
        map.get(session_id)
            .and_then(|entry| entry.terminal.as_ref().cloned())
    }

    pub fn list_terminal_sessions(&self) -> Vec<Arc<TerminalSession>> {
        let map = self.inner.read().unwrap();
        map.values()
            .filter_map(|entry| entry.terminal.as_ref().cloned())
            .collect()
    }

    fn remove_terminal(&self, session_id: &str) {
        let mut map = self.inner.write().unwrap();
        if let Some(entry) = map.get_mut(session_id) {
            entry.terminal = None;
        }
        map.retain(|_, entry| entry.terminal.is_some());
    }
}

pub struct RegistryGuard {
    registry: SessionRegistry,
    session_id: String,
}

impl Drop for RegistryGuard {
    fn drop(&mut self) {
        self.registry.remove_terminal(&self.session_id);
    }
}

static REGISTRY_INSTANCE: once_cell::sync::Lazy<SessionRegistry> =
    once_cell::sync::Lazy::new(SessionRegistry::new);

pub fn global_registry() -> &'static SessionRegistry {
    &REGISTRY_INSTANCE
}

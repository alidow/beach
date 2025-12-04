use std::sync::Arc;
use std::time::Instant;

use crate::assignment::AssignmentService;
use crate::bus_ingest;
use crate::persistence::PersistenceAdapter;
use crate::queue::ControllerQueue;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::time::SystemTime;
use tokio::task::JoinHandle;
use tracing::info;
use transport_bus::Bus;
use transport_unified_adapter::{UnifiedBusAdapter, UnifiedBusError};

#[derive(Clone)]
pub struct AppState {
    start: Instant,
    instance_id: String,
    assignment_enabled: bool,
    #[allow(dead_code)]
    queue: Arc<dyn ControllerQueue>,
    #[allow(dead_code)]
    persistence: Arc<dyn PersistenceAdapter>,
    #[allow(dead_code)]
    assignment: AssignmentService,
    bus_adapter: Option<Arc<dyn UnifiedBusAdapter>>,
    snapshot: SnapshotCache,
}

impl AppState {
    pub fn new(
        instance_id: String,
        assignment_enabled: bool,
        queue: Arc<dyn ControllerQueue>,
        persistence: Arc<dyn PersistenceAdapter>,
        assignment: AssignmentService,
        bus_adapter: Option<Arc<dyn UnifiedBusAdapter>>,
    ) -> Self {
        let snapshot = SnapshotCache::default();
        Self {
            start: Instant::now(),
            instance_id,
            assignment_enabled,
            queue,
            persistence,
            assignment,
            bus_adapter,
            snapshot,
        }
    }

    pub fn uptime_secs(&self) -> u64 {
        self.start.elapsed().as_secs()
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub fn assignment_enabled(&self) -> bool {
        self.assignment_enabled
    }

    pub fn queue(&self) -> Arc<dyn ControllerQueue> {
        Arc::clone(&self.queue)
    }

    #[allow(dead_code)]
    pub fn persistence(&self) -> Arc<dyn PersistenceAdapter> {
        Arc::clone(&self.persistence)
    }

    #[allow(dead_code)]
    pub fn assignment(&self) -> AssignmentService {
        self.assignment.clone()
    }

    /// Hook to attach a transport bus once the WebRTC shim is ready.
    #[allow(dead_code)]
    pub fn attach_bus(&self, bus: Arc<dyn Bus>) -> Vec<JoinHandle<()>> {
        bus_ingest::start_bus_ingest(bus, self.queue())
    }

    pub async fn attach_bus_for_host(&self, host_session_id: &str) -> Result<(), UnifiedBusError> {
        let Some(adapter) = &self.bus_adapter else {
            return Ok(());
        };
        info!(
            host_session_id,
            "attaching unified bus for host via configured adapter"
        );
        let bus = adapter.build_bus(host_session_id).await?;
        let _handles = self.attach_bus(bus);
        info!(host_session_id, "bus attached and ingest started");
        Ok(())
    }

    pub fn snapshot(&self) -> SnapshotCache {
        self.snapshot.clone()
    }
}

#[derive(Clone, Default)]
pub struct SnapshotCache {
    inner: Arc<Mutex<HashMap<String, CacheSnapshot>>>,
}

#[derive(Clone, Debug)]
pub struct CacheSnapshot {
    pub host_session_id: String,
    pub last_action_id: Option<String>,
    pub last_state_seq: Option<u64>,
    pub last_updated: SystemTime,
}

impl SnapshotCache {
    pub fn update_action(&self, host_session_id: &str, action_id: &str) {
        let mut guard = self.inner.lock();
        let entry = guard
            .entry(host_session_id.to_string())
            .or_insert(CacheSnapshot {
                host_session_id: host_session_id.to_string(),
                last_action_id: None,
                last_state_seq: None,
                last_updated: SystemTime::now(),
            });
        entry.last_action_id = Some(action_id.to_string());
        entry.last_updated = SystemTime::now();
    }

    pub fn update_state(&self, host_session_id: &str, seq: u64) {
        let mut guard = self.inner.lock();
        let entry = guard
            .entry(host_session_id.to_string())
            .or_insert(CacheSnapshot {
                host_session_id: host_session_id.to_string(),
                last_action_id: None,
                last_state_seq: None,
                last_updated: SystemTime::now(),
            });
        entry.last_state_seq = Some(seq);
        entry.last_updated = SystemTime::now();
    }

    pub fn get(&self, host_session_id: &str) -> Option<CacheSnapshot> {
        self.inner.lock().get(host_session_id).cloned()
    }
}

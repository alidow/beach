use std::sync::Arc;
use std::time::Instant;

use crate::assignment::AssignmentService;
use crate::bus_ingest;
use crate::persistence::PersistenceAdapter;
use crate::queue::ControllerQueue;
use tokio::task::JoinHandle;
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
        Self {
            start: Instant::now(),
            instance_id,
            assignment_enabled,
            queue,
            persistence,
            assignment,
            bus_adapter,
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
        let bus = adapter.build_bus(host_session_id).await?;
        let _handles = self.attach_bus(bus);
        Ok(())
    }
}

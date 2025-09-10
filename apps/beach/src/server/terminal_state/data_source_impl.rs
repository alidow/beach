use std::sync::{Arc, Mutex};
use async_trait::async_trait;
use anyhow::Result;
use tokio::sync::{mpsc, Mutex as TokioMutex};

use crate::protocol::Dimensions;
use crate::subscription::TerminalDataSource;
use super::{Grid, GridDelta, GridView, TerminalStateTracker, TerminalBackend};

/// Implementation of TerminalDataSource that wraps TerminalStateTracker
pub struct TrackerDataSource {
    tracker: Arc<Mutex<TerminalStateTracker>>,
    backend: Arc<Mutex<Box<dyn TerminalBackend>>>,
    delta_rx: Arc<TokioMutex<mpsc::Receiver<GridDelta>>>,
}

impl TrackerDataSource {
    /// Create a new data source from a terminal state tracker
    pub fn new(
        tracker: Arc<Mutex<TerminalStateTracker>>,
        backend: Arc<Mutex<Box<dyn TerminalBackend>>>,
    ) -> (Self, mpsc::Sender<GridDelta>) {
        // Create channel for receiving deltas
        let (delta_tx, delta_rx) = mpsc::channel(100);
        
        let source = Self {
            tracker,
            backend,
            delta_rx: Arc::new(TokioMutex::new(delta_rx)),
        };
        
        (source, delta_tx)
    }
}

#[async_trait]
impl TerminalDataSource for TrackerDataSource {
    async fn snapshot(&self, dims: Dimensions) -> Result<Grid> {
        // Get the current grid from the backend
        let backend = self.backend.lock().unwrap();
        let current_grid = backend.get_current_grid();
        drop(backend);
        
        // If dimensions match, return as-is
        if current_grid.width == dims.width && current_grid.height == dims.height {
            return Ok(current_grid);
        }
        
        // Otherwise, create a view with the requested dimensions
        let tracker = self.tracker.lock().unwrap();
        let history = tracker.get_history();
        drop(tracker);
        
        let mut view = GridView::new(history);
        let grid = view.derive_realtime(Some(dims.height))?;
        Ok(grid)
    }
    
    async fn next_delta(&self) -> Result<GridDelta> {
        // Wait for the next delta from the channel
        let mut rx = self.delta_rx.lock().await;
        match rx.recv().await {
            Some(delta) => Ok(delta),
            None => Err(anyhow::anyhow!("Delta channel closed")),
        }
    }
    
    async fn invalidate(&self) -> Result<()> {
        // Force the backend to refresh its state
        // This could trigger a full resync if needed
        Ok(())
    }
}
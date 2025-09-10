use std::sync::{Arc, Mutex};
use async_trait::async_trait;
use anyhow::Result;
use tokio::sync::{mpsc, Mutex as TokioMutex};
use chrono::{DateTime, Utc};

use crate::protocol::{Dimensions, ViewMode, ViewPosition};
use crate::subscription::{TerminalDataSource, HistoryMetadata};
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
    
    async fn snapshot_with_view(
        &self, 
        dims: Dimensions, 
        mode: ViewMode, 
        position: Option<ViewPosition>
    ) -> Result<Grid> {
        let tracker = self.tracker.lock().unwrap();
        let history = tracker.get_history();
        drop(tracker);
        
        let mut view = GridView::new(history);
        
        let grid = match mode {
            ViewMode::Realtime => {
                view.derive_realtime(Some(dims.height))?
            },
            ViewMode::Historical => {
                if let Some(pos) = position {
                    if let Some(line_num) = pos.line {
                        // Derive view from specific line number
                        view.derive_from_line(line_num, Some(dims.height))?
                    } else if let Some(timestamp) = pos.time {
                        // Derive view from specific timestamp
                        let dt = DateTime::<Utc>::from_timestamp(timestamp, 0)
                            .ok_or_else(|| anyhow::anyhow!("Invalid timestamp"))?;
                        view.derive_at_time(dt, Some(dims.height))?
                    } else {
                        // No position specified, default to current view
                        view.derive_realtime(Some(dims.height))?
                    }
                } else {
                    // No position specified, default to current view
                    view.derive_realtime(Some(dims.height))?
                }
            },
            ViewMode::Anchored => {
                // For now, anchored mode behaves like realtime
                // Future: implement anchoring to specific position
                view.derive_realtime(Some(dims.height))?
            },
        };
        
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
    
    async fn get_history_metadata(&self) -> Result<HistoryMetadata> {
        let tracker = self.tracker.lock().unwrap();
        let history = tracker.get_history();
        let history_guard = history.lock().unwrap();
        
        // Get current grid to determine line ranges
        let current_grid = history_guard.get_current()?;
        
        // Calculate metadata
        let latest_line = current_grid.end_line.to_u64().unwrap_or(0);
        let oldest_line = current_grid.start_line.to_u64()
            .unwrap_or(0)
            .saturating_sub(10000); // Default max history of 10,000 lines
        let total_lines = latest_line - oldest_line + 1;
        
        let metadata = HistoryMetadata {
            oldest_line,
            latest_line,
            total_lines,
            oldest_timestamp: None, // TODO: track oldest timestamp in GridHistory
            latest_timestamp: Some(current_grid.timestamp),
        };
        
        Ok(metadata)
    }
}
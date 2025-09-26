use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::{Arc, Mutex};
use tokio::sync::{Mutex as TokioMutex, mpsc};

use super::{Grid, GridDelta, GridView, TerminalBackend, TerminalStateTracker};
use crate::debug_recorder::{DebugEvent, DebugRecorder};
use crate::protocol::{Dimensions, ViewMode, ViewPosition};
use crate::subscription::{HistoryMetadata, TerminalDataSource};

/// Implementation of TerminalDataSource that wraps TerminalStateTracker
pub struct TrackerDataSource {
    tracker: Arc<Mutex<TerminalStateTracker>>,
    backend: Arc<Mutex<Box<dyn TerminalBackend>>>,
    delta_rx: Arc<TokioMutex<mpsc::Receiver<GridDelta>>>,
    debug_recorder: Option<Arc<Mutex<DebugRecorder>>>,
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
            debug_recorder: None,
        };

        (source, delta_tx)
    }

    /// Set the debug recorder
    pub fn set_debug_recorder(&mut self, recorder: Option<Arc<Mutex<DebugRecorder>>>) {
        self.debug_recorder = recorder;
    }
}

#[async_trait]
impl TerminalDataSource for TrackerDataSource {
    async fn snapshot(&self, dims: Dimensions) -> Result<Grid> {
        // Get the current grid from the backend
        let backend = self.backend.lock().unwrap();
        let backend_grid = backend.get_current_grid();
        drop(backend);

        // Get the grid from history reconstruction
        let tracker = self.tracker.lock().unwrap();
        let history = tracker.get_history();
        drop(tracker);

        let history_guard = history.lock().unwrap();
        let history_grid = history_guard.get_current()?;
        drop(history_guard);

        // Log bottom context for backend vs history
        if let Some(recorder) = &self.debug_recorder {
            if let Ok(mut rec) = recorder.try_lock() {
                let _ = rec.record_grid_bottom_context(
                    "server_data_source.backend_current",
                    &backend_grid,
                    6,
                );
                let _ = rec.record_grid_bottom_context(
                    "server_data_source.history_current",
                    &history_grid,
                    6,
                );
            }
        }

        // Log comparison between backend and history grids
        if let Some(recorder) = &self.debug_recorder {
            if let Ok(mut recorder) = recorder.try_lock() {
                // Collect differing lines
                let mut differing_lines = Vec::new();
                let mut difference_samples = Vec::new();

                let min_height = backend_grid.height.min(history_grid.height);
                let max_width = backend_grid.width.max(history_grid.width);

                for row in 0..min_height {
                    let mut backend_line = String::new();
                    let mut history_line = String::new();

                    for col in 0..max_width {
                        if col < backend_grid.width {
                            if let Some(cell) = backend_grid.get_cell(row, col) {
                                backend_line.push(cell.char);
                            }
                        }
                        if col < history_grid.width {
                            if let Some(cell) = history_grid.get_cell(row, col) {
                                history_line.push(cell.char);
                            }
                        }
                    }

                    if backend_line != history_line {
                        differing_lines.push(row);
                        if difference_samples.len() < 5 {
                            // Limit samples to first 5
                            difference_samples.push((
                                row,
                                backend_line.clone(),
                                history_line.clone(),
                            ));
                        }
                    }
                }

                let _ =
                    recorder.record_event(crate::debug_recorder::DebugEvent::SnapshotComparison {
                        timestamp: chrono::Utc::now(),
                        context: "TrackerDataSource::snapshot".to_string(),
                        backend_dims: (backend_grid.width, backend_grid.height),
                        history_dims: (history_grid.width, history_grid.height),
                        backend_blank_lines: backend_grid.count_blank_lines(),
                        history_blank_lines: history_grid.count_blank_lines(),
                        backend_content_dist: backend_grid.get_content_distribution(),
                        history_content_dist: history_grid.get_content_distribution(),
                        differing_lines,
                        difference_samples,
                    });
            }
        }

        // If dimensions match, return the history grid as-is
        if history_grid.width == dims.width && history_grid.height == dims.height {
            return Ok(history_grid);
        }

        // Otherwise, create a view with the requested dimensions
        let view = GridView::new(history.clone());
        let grid = view.derive_realtime(Some(dims.height))?;
        // Log bottom context for realtime snapshot
        if let Some(recorder) = &self.debug_recorder {
            if let Ok(mut rec) = recorder.try_lock() {
                let _ = rec.record_grid_bottom_context("server_data_source.snapshot", &grid, 6);
            }
        }
        Ok(grid)
    }

    async fn snapshot_with_view(
        &self,
        dims: Dimensions,
        mode: ViewMode,
        position: Option<ViewPosition>,
    ) -> Result<Grid> {
        // Debug event: SnapshotWithViewRequest
        if let Some(ref recorder) = self.debug_recorder {
            if let Ok(mut rec) = recorder.try_lock() {
                let _ = rec.record_event(DebugEvent::SnapshotWithViewRequest {
                    timestamp: chrono::Utc::now(),
                    mode: format!("{:?}", mode),
                    position_line: position.as_ref().and_then(|p| p.line),
                    position_time: position.as_ref().and_then(|p| p.time),
                    dimensions: (dims.width, dims.height),
                });
            }
        }

        let tracker = self.tracker.lock().unwrap();
        let history = tracker.get_history();
        drop(tracker);

        let view = GridView::new(history);

        // Debug log to file
        if let Ok(mut debug_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/beach-server-history.log")
        {
            use std::io::Write;
            let _ = writeln!(
                debug_file,
                "[{}] snapshot_with_view: mode={:?}, dims={}x{}, position={:?}",
                chrono::Utc::now(),
                mode,
                dims.width,
                dims.height,
                position
                    .as_ref()
                    .map(|p| format!("line={:?}, time={:?}", p.line, p.time))
            );
        }

        let grid = match mode {
            ViewMode::Realtime => view.derive_realtime(Some(dims.height))?,
            ViewMode::Historical => {
                if let Some(pos) = position {
                    if let Some(line_num) = pos.line {
                        // Log the historical line request
                        if let Some(ref recorder) = self.debug_recorder {
                            if let Ok(mut rec) = recorder.try_lock() {
                                let _ = rec.record_event(DebugEvent::Comment {
                                    timestamp: chrono::Utc::now(),
                                    text: format!(
                                        "DataSource: Historical view requested for line {}",
                                        line_num
                                    ),
                                });
                            }
                        }

                        // Debug log to file
                        if let Ok(mut debug_file) = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open("/tmp/beach-server-history.log")
                        {
                            use std::io::Write;
                            let _ = writeln!(
                                debug_file,
                                "[{}] DERIVING HISTORICAL VIEW: line_num={}, height={}",
                                chrono::Utc::now(),
                                line_num,
                                dims.height
                            );
                        }

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
            }
            ViewMode::Anchored => {
                // For now, anchored mode behaves like realtime
                // Future: implement anchoring to specific position
                view.derive_realtime(Some(dims.height))?
            }
        };

        // Debug log the result to file
        if let Ok(mut debug_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/beach-server-history.log")
        {
            use std::io::Write;
            // Count non-blank lines
            let mut non_blank_count = 0;
            let mut first_non_blank: Option<String> = None;
            for row in 0..grid.height {
                let mut line = String::new();
                for col in 0..grid.width.min(80) {
                    if let Some(cell) = grid.get_cell(row, col) {
                        line.push(cell.char);
                    } else {
                        line.push(' ');
                    }
                }
                let trimmed = line.trim_end();
                if !trimmed.is_empty() {
                    non_blank_count += 1;
                    if first_non_blank.is_none() {
                        first_non_blank = Some(trimmed.to_string());
                    }
                }
            }

            let _ = writeln!(
                debug_file,
                "[{}] RESULT: grid_dims={}x{}, start_line={:?}, end_line={:?}, non_blank_lines={}, first_non_blank={:?}",
                chrono::Utc::now(),
                grid.width,
                grid.height,
                grid.start_line.to_u64(),
                grid.end_line.to_u64(),
                non_blank_count,
                first_non_blank
            );
        }

        // Debug event: SnapshotWithViewResponse
        if let Some(ref recorder) = self.debug_recorder {
            if let Ok(mut rec) = recorder.try_lock() {
                // Collect sample content
                let mut sample_content = Vec::new();
                let mut blank_count = 0;
                for row in 0..grid.height.min(10) {
                    let mut line = String::new();
                    for col in 0..grid.width {
                        if let Some(cell) = grid.get_cell(row, col) {
                            line.push(cell.char);
                        } else {
                            line.push(' ');
                        }
                    }
                    let trimmed = line.trim_end();
                    if trimmed.is_empty() {
                        blank_count += 1;
                        sample_content.push(format!("Row {}: [BLANK]", row));
                    } else {
                        sample_content.push(format!("Row {}: {}", row, trimmed));
                    }
                }

                let _ = rec.record_event(DebugEvent::SnapshotWithViewResponse {
                    timestamp: chrono::Utc::now(),
                    mode: format!("{:?}", mode),
                    result_start_line: grid.start_line.to_u64().unwrap_or(0),
                    result_end_line: grid.end_line.to_u64().unwrap_or(0),
                    result_blank_count: blank_count,
                    sample_content,
                });

                // Also log bottom context for additional debugging
                let _ = rec.record_grid_bottom_context(
                    "server_data_source.snapshot_with_view",
                    &grid,
                    6,
                );
            }
        }
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
        let oldest_line = current_grid
            .start_line
            .to_u64()
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

    async fn snapshot_range_with_watermark(
        &self,
        width: u16,
        start_line: u64,
        rows: u16,
    ) -> Result<(Grid, u64)> {
        // Get tracker and history
        let tracker = self.tracker.lock().unwrap();
        let history = tracker.get_history();
        drop(tracker);

        let history_guard = history.lock().unwrap();

        // Get the current sequence number as watermark
        let watermark_seq = history_guard.get_current_sequence();

        // Create a grid view to derive the requested range
        let view = GridView::new(history.clone());
        drop(history_guard);

        // Derive grid for the requested line range
        let grid = view.derive_from_line(start_line, Some(rows))?;

        // Log debug info if recorder is set
        if let Some(ref recorder) = self.debug_recorder {
            if let Ok(mut rec) = recorder.try_lock() {
                let _ = rec.record_grid_bottom_context(
                    &format!(
                        "snapshot_range_watermark_w{}_s{}_r{}_wm{}",
                        width, start_line, rows, watermark_seq
                    ),
                    &grid,
                    3,
                );
            }
        }

        Ok((grid, watermark_seq))
    }
}

use crate::debug_recorder::{DebugEvent, DebugRecorder};
use crate::server::terminal_state::{Grid, GridDelta, LineCounter, TerminalStateError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Metadata about a snapshot for quick lookups
#[derive(Debug, Clone)]
struct SnapshotMeta {
    /// Sequence number of this snapshot
    seq: u64,
    /// Starting line number visible in this snapshot
    start_line: LineCounter,
    /// Ending line number visible in this snapshot
    end_line: LineCounter,
    /// Timestamp when snapshot was taken
    timestamp: DateTime<Utc>,
}

#[derive(Debug)]
pub struct GridHistory {
    /// Initial grid state
    initial_grid: Grid,

    /// Ordered deltas by sequence number
    deltas: BTreeMap<u64, GridDelta>,

    /// Snapshot grids for faster reconstruction
    snapshots: BTreeMap<u64, Grid>,

    /// Metadata about snapshots for efficient line-based lookup
    snapshot_meta: Vec<SnapshotMeta>,

    /// Index by timestamp for time-based lookup
    time_index: BTreeMap<DateTime<Utc>, u64>,

    /// Index by line number for line-based lookup
    line_index: BTreeMap<LineCounter, u64>,

    /// Current sequence number
    current_sequence: u64,

    /// Last snapshot timestamp (for rate limiting)
    last_snapshot_time: DateTime<Utc>,

    /// Configuration
    config: HistoryConfig,

    /// Optional debug recorder for logging delta applications
    debug_recorder: Option<Arc<Mutex<DebugRecorder>>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryConfig {
    /// Interval between snapshots (in delta count)
    pub snapshot_interval: u64,

    /// Minimum time between snapshots (milliseconds) to prevent CPU thrashing
    pub min_snapshot_interval_ms: u64,

    /// Maximum history size in bytes
    pub max_size_bytes: usize,

    /// Maximum number of lines to retain in history (0 = unlimited)
    pub max_history_lines: u64,

    /// Compression settings
    pub enable_compression: bool,

    /// Delta coalescing window (milliseconds)
    pub coalesce_window_ms: u64,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        HistoryConfig {
            snapshot_interval: 100,         // Snapshot every 100 deltas
            min_snapshot_interval_ms: 5000, // At least 5 seconds between snapshots
            max_size_bytes: 100_000_000,    // 100MB limit
            max_history_lines: 10000,       // Keep 10,000 lines of history
            enable_compression: true,
            coalesce_window_ms: 50, // Coalesce changes within 50ms
        }
    }
}

impl GridHistory {
    #[cfg(test)]
    pub fn clear_deltas(&mut self) {
        self.deltas.clear();
    }

    #[cfg(test)]
    pub fn delta_count(&self) -> usize {
        self.deltas.len()
    }

    #[cfg(test)]
    pub fn has_deltas(&self) -> bool {
        !self.deltas.is_empty()
    }

    #[cfg(test)]
    pub fn iter_deltas(&self) -> impl Iterator<Item = &GridDelta> {
        self.deltas.values()
    }

    #[cfg(test)]
    pub fn current_grid_mut(&mut self) -> &mut Grid {
        &mut self.initial_grid
    }

    pub fn new(initial_grid: Grid) -> Self {
        Self::new_with_debug(initial_grid, None)
    }

    pub fn new_with_debug(
        initial_grid: Grid,
        debug_recorder: Option<Arc<Mutex<DebugRecorder>>>,
    ) -> Self {
        let mut history = GridHistory {
            initial_grid: initial_grid.clone(),
            deltas: BTreeMap::new(),
            snapshots: BTreeMap::new(),
            snapshot_meta: Vec::new(),
            time_index: BTreeMap::new(),
            line_index: BTreeMap::new(),
            current_sequence: 0,
            last_snapshot_time: initial_grid.timestamp,
            config: HistoryConfig::default(),
            debug_recorder,
        };

        // Add initial grid as first snapshot with metadata
        history.snapshots.insert(0, initial_grid.clone());
        history.snapshot_meta.push(SnapshotMeta {
            seq: 0,
            start_line: initial_grid.start_line.clone(),
            end_line: initial_grid.end_line.clone(),
            timestamp: initial_grid.timestamp,
        });
        history.time_index.insert(initial_grid.timestamp, 0);
        history
            .line_index
            .insert(initial_grid.start_line.clone(), 0);

        history
    }

    /// Add a new delta to history
    pub fn add_delta(&mut self, mut delta: GridDelta) {
        self.current_sequence += 1;
        delta.sequence = self.current_sequence;

        // Add indexes
        self.time_index.insert(delta.timestamp, delta.sequence);

        // Add delta
        self.deltas.insert(delta.sequence, delta.clone());

        // Check if we need a snapshot (respecting both count and time intervals)
        let should_snapshot = self.current_sequence % self.config.snapshot_interval == 0
            && delta
                .timestamp
                .signed_duration_since(self.last_snapshot_time)
                .num_milliseconds()
                >= self.config.min_snapshot_interval_ms as i64;

        if should_snapshot {
            self.create_snapshot();
            self.last_snapshot_time = delta.timestamp;
        }

        // Check memory limits
        self.enforce_memory_limits();
    }

    /// Add a snapshot of the current grid state
    pub fn add_snapshot(&mut self, grid: Grid) {
        // Add snapshot metadata for efficient lookup
        self.snapshot_meta.push(SnapshotMeta {
            seq: self.current_sequence,
            start_line: grid.start_line.clone(),
            end_line: grid.end_line.clone(),
            timestamp: grid.timestamp,
        });

        self.snapshots.insert(self.current_sequence, grid.clone());
        self.line_index
            .insert(grid.start_line.clone(), self.current_sequence);
        self.last_snapshot_time = grid.timestamp;
    }

    /// Get current grid state
    pub fn get_current(&self) -> Result<Grid, TerminalStateError> {
        self.reconstruct_from_sequence(self.current_sequence)
    }

    /// Get the current sequence number (watermark)
    pub fn get_current_sequence(&self) -> u64 {
        self.current_sequence
    }

    /// Get grid state at a specific timestamp
    pub fn get_at_time(&self, timestamp: DateTime<Utc>) -> Result<Grid, TerminalStateError> {
        // Find the sequence number at or before the given timestamp
        let target_seq = self
            .time_index
            .range(..=timestamp)
            .rev()
            .next()
            .map(|(_, seq)| *seq)
            .unwrap_or(0); // If no entry found, use initial state

        self.reconstruct_from_sequence(target_seq)
    }

    /// Get grid state containing a specific line number
    /// Returns a grid from history whose visible window contains the requested line
    pub fn get_from_line(&self, line_num: u64) -> Result<Grid, TerminalStateError> {
        let _target_line = LineCounter::from_u64(line_num);

        // Debug event: HistoryLookupRequested
        if let Some(ref recorder) = self.debug_recorder {
            if let Ok(mut rec) = recorder.try_lock() {
                let _ = rec.record_event(DebugEvent::HistoryLookupRequested {
                    timestamp: Utc::now(),
                    requested_line: line_num,
                });
            }
        }

        // Find the best snapshot containing this line using binary search
        let mut best_snapshot: Option<&SnapshotMeta> = None;

        // First, try to find a snapshot that contains the line within its window
        for meta in &self.snapshot_meta {
            let start_u64 = meta.start_line.to_u64().unwrap_or(0);
            let end_u64 = meta.end_line.to_u64().unwrap_or(0);

            if line_num >= start_u64 && line_num <= end_u64 {
                // Found a snapshot containing the line
                best_snapshot = Some(meta);
                break;
            }
        }

        // If no exact match, find the closest snapshot before the line
        if best_snapshot.is_none() {
            for meta in &self.snapshot_meta {
                let start_u64 = meta.start_line.to_u64().unwrap_or(0);

                if start_u64 <= line_num {
                    best_snapshot = Some(meta);
                    // Continue searching for a closer one
                } else {
                    // We've gone past the target line
                    break;
                }
            }
        }

        // If still no snapshot found, use the initial grid
        let seq_start = best_snapshot.map(|m| m.seq).unwrap_or(0);

        // Debug event: HistoryLookupCandidate
        if let Some(ref recorder) = self.debug_recorder {
            if let Ok(mut rec) = recorder.try_lock() {
                let snapshot_start = best_snapshot
                    .map(|m| m.start_line.to_u64().unwrap_or(0))
                    .unwrap_or(0);
                let snapshot_end = best_snapshot
                    .map(|m| m.end_line.to_u64().unwrap_or(0))
                    .unwrap_or(self.initial_grid.height as u64 - 1);

                let _ = rec.record_event(DebugEvent::HistoryLookupCandidate {
                    timestamp: Utc::now(),
                    snapshot_index: best_snapshot.map(|_| self.snapshots.len() - 1).unwrap_or(0),
                    snapshot_start_line: snapshot_start,
                    snapshot_end_line: snapshot_end,
                    contains_line: line_num >= snapshot_start && line_num <= snapshot_end,
                });
            }
        }

        // Reconstruct from the chosen snapshot
        if let Ok(mut debug_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/beach-history-debug.log")
        {
            use std::io::Write;
            let _ = writeln!(
                debug_file,
                "[{}] GET_FROM_LINE requesting line={} found_seq={} current_seq={}",
                chrono::Utc::now().format("%H:%M:%S%.3f"),
                line_num,
                seq_start,
                self.current_sequence
            );
        }

        // Instead of using the snapshot sequence, always reconstruct from the CURRENT sequence
        // to get the most up-to-date content. Historical lines don't change, but the grid
        // grows over time, so we need current content to find the requested line.
        let mut grid = self.reconstruct_from_sequence(self.current_sequence)?;

        if let Ok(mut debug_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/beach-history-debug.log")
        {
            use std::io::Write;
            let grid_start = grid.start_line.to_u64().unwrap_or(0);
            let grid_end = grid.end_line.to_u64().unwrap_or(0);
            let _ = writeln!(
                debug_file,
                "[{}] GET_FROM_LINE reconstructed current grid range=({},{}) dims={}x{} for line={}",
                chrono::Utc::now().format("%H:%M:%S%.3f"),
                grid_start,
                grid_end,
                grid.width,
                grid.height,
                line_num
            );
        }

        // Final check and debug logging
        let final_start = grid.start_line.to_u64().unwrap_or(0);
        let final_end = grid.end_line.to_u64().unwrap_or(0);
        let contains_target = line_num >= final_start && line_num <= final_end;

        // Count applied deltas
        let applied_deltas = if seq_start < self.current_sequence {
            self.deltas
                .range(seq_start + 1..=self.current_sequence)
                .count() as u64
        } else {
            0
        };

        // Debug event: HistoryReconstructEnd
        if let Some(ref recorder) = self.debug_recorder {
            if let Ok(mut rec) = recorder.try_lock() {
                let _ = rec.record_event(DebugEvent::HistoryReconstructEnd {
                    timestamp: Utc::now(),
                    target_line: line_num,
                    found_snapshot: true,
                    result_start_line: Some(final_start),
                    result_end_line: Some(final_end),
                    result_blank_count: Some(grid.count_blank_lines()),
                });
            }
        }

        // Debug: log what historical grid content is being returned
        if let Ok(mut debug_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/beach-history-debug.log")
        {
            use std::io::Write;
            let mut sample_content = Vec::new();
            let mut non_blank_rows = 0;

            // Sample first few rows to see actual content
            for row in 0..grid.height.min(5) {
                let mut line = String::new();
                for col in 0..grid.width.min(80) {
                    if let Some(cell) = grid.get_cell(row, col) {
                        line.push(cell.char);
                    }
                }
                let trimmed = line.trim_end();
                if !trimmed.is_empty() {
                    non_blank_rows += 1;
                }
                sample_content.push(format!("row[{}]: '{}'", row, trimmed));
            }

            let _ = writeln!(
                debug_file,
                "[{}] HISTORY_RETRIEVED for line={} grid_range=({},{}) dims={}x{} non_blank={}/5 content=[{}]",
                chrono::Utc::now().format("%H:%M:%S%.3f"),
                line_num,
                final_start,
                final_end,
                grid.width,
                grid.height,
                non_blank_rows,
                sample_content.join(", ")
            );
        }

        Ok(grid)
    }

    /// Reconstruct grid from nearest snapshot
    fn reconstruct_from_sequence(&self, target_seq: u64) -> Result<Grid, TerminalStateError> {
        // Find nearest snapshot at or before target, or use initial grid
        let (snapshot_seq, mut grid) = self
            .snapshots
            .range(..=target_seq)
            .rev()
            .next()
            .map(|(seq, grid)| (*seq, grid.clone()))
            .unwrap_or((0, self.initial_grid.clone()));

        // Debug event: ReconstructionPath
        if let Some(ref recorder) = self.debug_recorder {
            if let Ok(mut rec) = recorder.try_lock() {
                let delta_count = if snapshot_seq < target_seq {
                    self.deltas.range(snapshot_seq + 1..=target_seq).count() as u64
                } else {
                    0
                };

                let _ = rec.record_event(DebugEvent::ReconstructionPath {
                    timestamp: Utc::now(),
                    starting_snapshot_index: snapshot_seq as usize,
                    starting_line: grid.start_line.to_u64().unwrap_or(0),
                    deltas_applied: delta_count as usize,
                    final_line: grid.end_line.to_u64().unwrap_or(0),
                });
            }
        }

        // Log the starting point for reconstruction
        if let Some(ref recorder) = self.debug_recorder {
            if let Ok(mut rec) = recorder.try_lock() {
                let _ = rec.record_event(DebugEvent::Comment {
                    timestamp: Utc::now(),
                    text: format!(
                        "reconstruct_from_sequence: Starting from seq {} (target {}), grid dims {}x{}, blank lines: {}",
                        snapshot_seq, target_seq, grid.width, grid.height, grid.count_blank_lines()
                    ),
                });

                // Log content distribution at start
                let content_dist = grid.get_content_distribution();
                let content_lines: Vec<u16> = content_dist
                    .iter()
                    .filter(|(_, has_content)| *has_content)
                    .map(|(row, _)| *row)
                    .collect();
                let _ = rec.record_event(DebugEvent::Comment {
                    timestamp: Utc::now(),
                    text: format!(
                        "Starting content distribution: {} content lines at rows {:?}",
                        content_lines.len(),
                        &content_lines[..content_lines.len().min(10)] // Show first 10
                    ),
                });
            }
        }

        // Apply deltas from snapshot to target
        if snapshot_seq < target_seq {
            let delta_count = self.deltas.range(snapshot_seq + 1..=target_seq).count();

            // Log delta application summary
            if let Some(ref recorder) = self.debug_recorder {
                if let Ok(mut rec) = recorder.try_lock() {
                    let _ = rec.record_event(DebugEvent::Comment {
                        timestamp: Utc::now(),
                        text: format!(
                            "Applying {} deltas from seq {} to {}",
                            delta_count,
                            snapshot_seq + 1,
                            target_seq
                        ),
                    });
                }
            }

            for (seq, delta) in self.deltas.range(snapshot_seq + 1..=target_seq) {
                // Log delta application if debug recorder is available
                if let Some(ref recorder) = self.debug_recorder {
                    self.log_delta_application(recorder, &grid, delta, "reconstruct_from_sequence");

                    // Log pre-application state for critical deltas
                    if delta.cell_changes.len() > 100 || delta.dimension_change.is_some() {
                        if let Ok(mut rec) = recorder.try_lock() {
                            let _ = rec.record_event(DebugEvent::Comment {
                                timestamp: Utc::now(),
                                text: format!(
                                    "Large delta at seq {}: {} cell changes, dimension change: {:?}",
                                    seq, delta.cell_changes.len(), delta.dimension_change.is_some()
                                ),
                            });
                        }
                    }
                }

                let blank_lines_before = grid.count_blank_lines();
                delta.apply(&mut grid)?;
                let blank_lines_after = grid.count_blank_lines();

                // Log if blank lines changed significantly
                if blank_lines_after != blank_lines_before {
                    if let Some(ref recorder) = self.debug_recorder {
                        if let Ok(mut rec) = recorder.try_lock() {
                            let _ = rec.record_event(DebugEvent::Comment {
                                timestamp: Utc::now(),
                                text: format!(
                                    "Delta {} changed blank lines: {} -> {} (diff: {})",
                                    seq,
                                    blank_lines_before,
                                    blank_lines_after,
                                    blank_lines_after as i32 - blank_lines_before as i32
                                ),
                            });

                            // Log content distribution if there's a significant change
                            if (blank_lines_after as i32 - blank_lines_before as i32).abs() > 3 {
                                let content_dist = grid.get_content_distribution();
                                let blank_rows: Vec<u16> = content_dist
                                    .iter()
                                    .filter(|(_, has_content)| !*has_content)
                                    .map(|(row, _)| *row)
                                    .collect();
                                let _ = rec.record_event(DebugEvent::Comment {
                                    timestamp: Utc::now(),
                                    text: format!(
                                        "New blank rows at: {:?}",
                                        &blank_rows[..blank_rows.len().min(20)] // Show first 20
                                    ),
                                });
                            }
                        }
                    }
                }
            }
        }

        // Log final state
        if let Some(ref recorder) = self.debug_recorder {
            if let Ok(mut rec) = recorder.try_lock() {
                let _ = rec.record_event(DebugEvent::Comment {
                    timestamp: Utc::now(),
                    text: format!(
                        "reconstruct_from_sequence: Completed. Final grid {}x{}, blank lines: {}",
                        grid.width,
                        grid.height,
                        grid.count_blank_lines()
                    ),
                });
            }
        }

        Ok(grid)
    }

    /// Create a new snapshot at current state
    fn create_snapshot(&mut self) {
        if let Ok(grid) = self.get_current() {
            // Add snapshot metadata for efficient lookup
            self.snapshot_meta.push(SnapshotMeta {
                seq: self.current_sequence,
                start_line: grid.start_line.clone(),
                end_line: grid.end_line.clone(),
                timestamp: grid.timestamp,
            });

            self.snapshots.insert(self.current_sequence, grid.clone());
            self.line_index
                .insert(grid.start_line.clone(), self.current_sequence);
        }
    }

    /// Enforce memory limits using a modified ring buffer approach
    fn enforce_memory_limits(&mut self) {
        let estimated_size = self.estimate_memory_usage();

        if estimated_size > self.config.max_size_bytes {
            // When we exceed the limit, recalculate the initial grid to be the current state
            if let Ok(current_grid) = self.get_current() {
                // Clear all history
                self.deltas.clear();
                self.snapshots.clear();
                self.snapshot_meta.clear(); // Clear metadata too
                self.time_index.clear();
                self.line_index.clear();

                // Reset with current grid as new initial state
                self.initial_grid = current_grid.clone();
                self.current_sequence = 0;
                self.last_snapshot_time = current_grid.timestamp;

                // Add the new initial grid as first snapshot with metadata
                self.snapshots.insert(0, current_grid.clone());
                self.snapshot_meta.push(SnapshotMeta {
                    seq: 0,
                    start_line: current_grid.start_line.clone(),
                    end_line: current_grid.end_line.clone(),
                    timestamp: current_grid.timestamp,
                });
                self.time_index.insert(current_grid.timestamp, 0);
                self.line_index.insert(current_grid.start_line.clone(), 0);
            }
        }
    }

    fn estimate_memory_usage(&self) -> usize {
        // Rough estimation
        let grid_size = 80 * 24 * 24; // width * height * bytes_per_cell
        let snapshot_size = self.snapshots.len() * grid_size;
        let delta_size = self.deltas.len() * 200; // Average delta size
        snapshot_size + delta_size
    }

    /// Log delta application for debugging
    fn log_delta_application(
        &self,
        recorder: &Arc<Mutex<DebugRecorder>>,
        grid: &Grid,
        delta: &GridDelta,
        context: &str,
    ) {
        // Count blank lines before applying delta
        let blank_lines_before = self.count_blank_lines(grid);

        // Get sample of grid content before (first 5 lines)
        let content_before: Vec<String> = (0..5.min(grid.height))
            .map(|row| self.get_row_text(grid, row))
            .collect();

        // Collect modified lines from cell changes
        let mut modified_lines: Vec<u16> = delta.cell_changes.iter().map(|c| c.row).collect();
        modified_lines.sort();
        modified_lines.dedup();

        // Create a temporary grid to count blank lines after
        let mut temp_grid = grid.clone();
        let _ = delta.apply(&mut temp_grid);
        let blank_lines_after = self.count_blank_lines(&temp_grid);

        // Get sample of grid content after (first 5 lines)
        let content_after: Vec<String> = (0..5.min(temp_grid.height))
            .map(|row| self.get_row_text(&temp_grid, row))
            .collect();

        // Prepare dimension change info
        let dimension_change = delta
            .dimension_change
            .as_ref()
            .map(|dc| (dc.old_width, dc.old_height, dc.new_width, dc.new_height));

        // Create and record the debug event
        let event = DebugEvent::GridDeltaApplication {
            timestamp: Utc::now(),
            context: context.to_string(),
            sequence: delta.sequence,
            cell_changes_count: delta.cell_changes.len(),
            modified_lines,
            has_dimension_change: delta.dimension_change.is_some(),
            dimension_change,
            has_cursor_change: delta.cursor_change.is_some(),
            before_dims: (grid.width, grid.height),
            after_dims: (temp_grid.width, temp_grid.height),
            blank_lines_before,
            blank_lines_after,
            content_before,
            content_after,
        };

        if let Ok(mut rec) = recorder.try_lock() {
            let _ = rec.record_event(event);
        }
    }

    /// Count blank lines in a grid
    fn count_blank_lines(&self, grid: &Grid) -> usize {
        let mut count = 0;
        for row in 0..grid.height {
            if self.is_row_blank(grid, row) {
                count += 1;
            }
        }
        count
    }

    /// Check if a row is blank (all spaces or default characters)
    fn is_row_blank(&self, grid: &Grid, row: u16) -> bool {
        for col in 0..grid.width {
            if let Some(cell) = grid.get_cell(row, col) {
                if cell.char != ' ' && cell.char != '\0' {
                    return false;
                }
            }
        }
        true
    }

    /// Get text content of a row
    fn get_row_text(&self, grid: &Grid, row: u16) -> String {
        let mut text = String::new();
        for col in 0..grid.width {
            if let Some(cell) = grid.get_cell(row, col) {
                if cell.char != '\0' {
                    text.push(cell.char);
                } else {
                    text.push(' ');
                }
            } else {
                text.push(' ');
            }
        }
        // Trim trailing spaces for readability
        text.trim_end().to_string()
    }

    /// Get statistics about the history
    pub fn get_stats(&self) -> HistoryStats {
        let memory_usage = self.estimate_memory_usage();
        let total_deltas = self.current_sequence;
        let total_snapshots = self.snapshots.len();

        // Calculate session duration
        let start_time = self.initial_grid.timestamp;
        let end_time = if let Some((timestamp, _)) = self.time_index.iter().rev().next() {
            *timestamp
        } else {
            start_time
        };
        let session_duration = end_time
            .signed_duration_since(start_time)
            .to_std()
            .unwrap_or_else(|_| std::time::Duration::from_secs(0));

        HistoryStats {
            memory_usage,
            total_deltas,
            total_snapshots,
            session_duration,
        }
    }

    /// Clear all history, keeping only the current state
    pub fn clear(&mut self) {
        if let Ok(current_grid) = self.get_current() {
            // Clear all history
            self.deltas.clear();
            self.snapshots.clear();
            self.snapshot_meta.clear(); // Clear metadata too
            self.time_index.clear();
            self.line_index.clear();

            // Reset with current grid as new initial state
            self.initial_grid = current_grid.clone();
            self.current_sequence = 0;
            self.last_snapshot_time = current_grid.timestamp;

            // Add the new initial grid as first snapshot with metadata
            self.snapshots.insert(0, current_grid.clone());
            self.snapshot_meta.push(SnapshotMeta {
                seq: 0,
                start_line: current_grid.start_line.clone(),
                end_line: current_grid.end_line.clone(),
                timestamp: current_grid.timestamp,
            });
            self.time_index.insert(current_grid.timestamp, 0);
            self.line_index.insert(current_grid.start_line.clone(), 0);
        }
    }
}

/// Statistics about the history
pub struct HistoryStats {
    pub memory_usage: usize,
    pub total_deltas: u64,
    pub total_snapshots: usize,
    pub session_duration: std::time::Duration,
}

use chrono::{DateTime, Duration, Utc};
use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use crate::server::terminal_state::{Grid, GridDelta, LineCounter, TerminalStateError};

#[derive(Debug)]
pub struct GridHistory {
    /// Initial grid state
    initial_grid: Grid,
    
    /// Ordered deltas by sequence number
    deltas: BTreeMap<u64, GridDelta>,
    
    /// Snapshot grids for faster reconstruction
    snapshots: BTreeMap<u64, Grid>,
    
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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryConfig {
    /// Interval between snapshots (in delta count)
    pub snapshot_interval: u64,
    
    /// Minimum time between snapshots (milliseconds) to prevent CPU thrashing
    pub min_snapshot_interval_ms: u64,
    
    /// Maximum history size in bytes
    pub max_size_bytes: usize,
    
    /// Compression settings
    pub enable_compression: bool,
    
    /// Delta coalescing window (milliseconds)
    pub coalesce_window_ms: u64,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        HistoryConfig {
            snapshot_interval: 100,        // Snapshot every 100 deltas
            min_snapshot_interval_ms: 5000, // At least 5 seconds between snapshots
            max_size_bytes: 100_000_000,  // 100MB limit
            enable_compression: true,
            coalesce_window_ms: 50,       // Coalesce changes within 50ms
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
        let mut history = GridHistory {
            initial_grid: initial_grid.clone(),
            deltas: BTreeMap::new(),
            snapshots: BTreeMap::new(),
            time_index: BTreeMap::new(),
            line_index: BTreeMap::new(),
            current_sequence: 0,
            last_snapshot_time: initial_grid.timestamp,
            config: HistoryConfig::default(),
        };
        
        // Add initial grid as first snapshot
        history.snapshots.insert(0, initial_grid.clone());
        history.time_index.insert(initial_grid.timestamp, 0);
        history.line_index.insert(initial_grid.start_line.clone(), 0);
        
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
            && delta.timestamp.signed_duration_since(self.last_snapshot_time).num_milliseconds() 
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
        self.snapshots.insert(self.current_sequence, grid.clone());
        self.line_index.insert(grid.start_line.clone(), self.current_sequence);
        self.last_snapshot_time = grid.timestamp;
    }
    
    /// Get current grid state
    pub fn get_current(&self) -> Result<Grid, TerminalStateError> {
        self.reconstruct_from_sequence(self.current_sequence)
    }
    
    /// Get grid state at a specific timestamp
    pub fn get_at_time(&self, timestamp: DateTime<Utc>) -> Result<Grid, TerminalStateError> {
        // Find the sequence number at or before the given timestamp
        let target_seq = self.time_index
            .range(..=timestamp)
            .rev()
            .next()
            .map(|(_, seq)| *seq)
            .unwrap_or(0); // If no entry found, use initial state
        
        self.reconstruct_from_sequence(target_seq)
    }
    
    /// Get grid state containing a specific line number
    /// Note: This returns the grid that contains the line, not necessarily starting at the line
    pub fn get_from_line(&self, line_num: u64) -> Result<Grid, TerminalStateError> {
        // For now, just return the current grid
        // The view shifting is handled by GridView::derive_from_line
        self.get_current()
    }
    
    /// Reconstruct grid from nearest snapshot
    fn reconstruct_from_sequence(&self, target_seq: u64) -> Result<Grid, TerminalStateError> {
        // Find nearest snapshot at or before target, or use initial grid
        let (snapshot_seq, mut grid) = self.snapshots
            .range(..=target_seq)
            .rev()
            .next()
            .map(|(seq, grid)| (*seq, grid.clone()))
            .unwrap_or((0, self.initial_grid.clone()));
        
        // Apply deltas from snapshot to target
        if snapshot_seq < target_seq {
            for (_, delta) in self.deltas.range(snapshot_seq + 1..=target_seq) {
                delta.apply(&mut grid)?;
            }
        }
        
        Ok(grid)
    }
    
    /// Create a new snapshot at current state
    fn create_snapshot(&mut self) {
        if let Ok(grid) = self.get_current() {
            self.snapshots.insert(self.current_sequence, grid.clone());
            self.line_index.insert(grid.start_line.clone(), self.current_sequence);
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
                self.time_index.clear();
                self.line_index.clear();
                
                // Reset with current grid as new initial state
                self.initial_grid = current_grid.clone();
                self.current_sequence = 0;
                self.last_snapshot_time = current_grid.timestamp;
                
                // Add the new initial grid as first snapshot
                self.snapshots.insert(0, current_grid.clone());
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
        let session_duration = end_time.signed_duration_since(start_time)
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
            self.time_index.clear();
            self.line_index.clear();
            
            // Reset with current grid as new initial state
            self.initial_grid = current_grid.clone();
            self.current_sequence = 0;
            self.last_snapshot_time = current_grid.timestamp;
            
            // Add the new initial grid as first snapshot
            self.snapshots.insert(0, current_grid.clone());
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

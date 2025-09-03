# Terminal State Tracking - Technical Design Document

## Executive Summary

This document outlines the technical design for a comprehensive terminal state tracking system that enables:
- Full terminal history preservation
- Time-travel debugging capabilities
- Dimension-aware re-wrapping
- Efficient memory usage through delta compression
- Fast lookups by line number, absolute time, or relative time

## System Architecture

### Core Components Overview

```
┌─────────────────────────────────────────────────────────────┐
│                     Terminal State System                      │
├─────────────────────────────────────────────────────────────┤
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐   │
│  │   Cell   │  │   Grid   │  │  Delta   │  │ History  │   │
│  └──────────┘  └──────────┘  └──────────┘  └──────────┘   │
│                                                               │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │
│  │ Line Counter │  │ Char Counter │  │  Grid View   │      │
│  └──────────────┘  └──────────────┘  └──────────────┘      │
└─────────────────────────────────────────────────────────────┘
```

## Detailed Component Specifications

### 1. Cell (cell.rs)

Represents a single terminal cell with all its visual attributes.

```rust
use serde::{Serialize, Deserialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Cell {
    /// Unicode character (can be multi-byte)
    pub char: char,
    
    /// Foreground color (24-bit RGB or indexed)
    pub fg_color: Color,
    
    /// Background color (24-bit RGB or indexed)
    pub bg_color: Color,
    
    /// Text attributes
    pub attributes: CellAttributes,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Color {
    Default,
    Indexed(u8),           // 256-color palette
    Rgb(u8, u8, u8),      // 24-bit true color
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CellAttributes {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub reverse: bool,
    pub blink: bool,
    pub dim: bool,
    pub hidden: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            char: ' ',
            fg_color: Color::Default,
            bg_color: Color::Default,
            attributes: CellAttributes::default(),
        }
    }
}

impl Cell {
    /// Memory-efficient serialization for storage
    pub fn to_bytes(&self) -> Vec<u8> {
        // Compact binary representation
        // Format: [char:4][fg:4][bg:4][attrs:1]
        let mut bytes = Vec::with_capacity(13);
        
        // Encode char as UTF-8 (1-4 bytes)
        let char_bytes = self.char.to_string().into_bytes();
        bytes.push(char_bytes.len() as u8);
        bytes.extend_from_slice(&char_bytes);
        
        // Encode colors (3-4 bytes each)
        bytes.extend(self.fg_color.to_bytes());
        bytes.extend(self.bg_color.to_bytes());
        
        // Pack attributes into single byte
        bytes.push(self.attributes.to_byte());
        
        bytes
    }
    
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        // Deserialize from compact format
        // Implementation details...
    }
}
```

### 2. Line Counter (line_counter.rs)

Tracks absolute line position in terminal history using BigInt for unlimited range.

```rust
use num_bigint::BigUint;
use serde::{Serialize, Deserialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct LineCounter {
    /// Unwrapped line number (dimension-independent)
    value: BigUint,
}

impl LineCounter {
    pub fn new() -> Self {
        LineCounter { value: BigUint::from(0u32) }
    }
    
    pub fn from_u64(val: u64) -> Self {
        LineCounter { value: BigUint::from(val) }
    }
    
    pub fn increment(&mut self) {
        self.value += 1u32;
    }
    
    pub fn add(&mut self, lines: u64) {
        self.value += lines;
    }
    
    /// Calculate wrapped line for given width
    pub fn to_wrapped(&self, content: &str, width: u16) -> u64 {
        // Account for line wrapping at specific width
        let mut wrapped_lines = 0u64;
        for line in content.lines() {
            let line_width = unicode_width::UnicodeWidthStr::width(line);
            wrapped_lines += ((line_width as u64) + (width as u64) - 1) / (width as u64);
        }
        wrapped_lines
    }
}
```

### 3. Char Counter (char_counter.rs)

Tracks absolute character position including non-visible characters.

```rust
use num_bigint::BigUint;
use serde::{Serialize, Deserialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct CharCounter {
    /// Total character count including control chars
    value: BigUint,
}

impl CharCounter {
    pub fn new() -> Self {
        CharCounter { value: BigUint::from(0u32) }
    }
    
    pub fn increment(&mut self, count: usize) {
        self.value += count;
    }
    
    pub fn get(&self) -> &BigUint {
        &self.value
    }
}
```

### 4. Grid (grid.rs)

Fixed-dimension snapshot of terminal state at a point in time.

```rust
use chrono::{DateTime, Utc};
use serde::{Serialize, Deserialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Grid {
    /// Grid dimensions at capture time
    pub width: u16,
    pub height: u16,
    
    /// 2D array of cells [row][col]
    pub cells: Vec<Vec<Cell>>,
    
    /// Line number at top of grid
    pub start_line: LineCounter,
    
    /// Line number at bottom of grid
    pub end_line: LineCounter,
    
    /// Cursor position (may be hidden)
    pub cursor: CursorPosition,
    
    /// Timestamp when grid was captured
    pub timestamp: DateTime<Utc>,
    
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CursorPosition {
    pub row: u16,
    pub col: u16,
    pub visible: bool,
    pub shape: CursorShape,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CursorShape {
    Block,
    Underline,
    Bar,
}

impl Grid {
    pub fn new(width: u16, height: u16) -> Self {
        Grid {
            width,
            height,
            cells: vec![vec![Cell::default(); width as usize]; height as usize],
            start_line: LineCounter::new(),
            end_line: LineCounter::from_u64(height as u64 - 1),
            cursor: CursorPosition {
                row: 0,
                col: 0,
                visible: true,
                shape: CursorShape::Block,
            },
            timestamp: Utc::now()
        }
    }
    
    /// Get cell at position
    pub fn get_cell(&self, row: u16, col: u16) -> Option<&Cell> {
        self.cells.get(row as usize)?.get(col as usize)
    }
    
    /// Set cell at position
    pub fn set_cell(&mut self, row: u16, col: u16, cell: Cell) {
        if let Some(row_cells) = self.cells.get_mut(row as usize) {
            if let Some(target_cell) = row_cells.get_mut(col as usize) {
                *target_cell = cell;
            }
        }
    }
    
    /// Compress grid for efficient storage
    pub fn compress(&self) -> CompressedGrid {
        // Use zstd or similar for compression
        // Group identical cells, use RLE encoding
        CompressedGrid {
            // Implementation details...
        }
    }
}
```

### 5. Grid Delta (grid_delta.rs)

Efficient representation of changes between grid states.

```rust
use chrono::{DateTime, Utc};
use serde::{Serialize, Deserialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GridDelta {
    /// Timestamp of change
    pub timestamp: DateTime<Utc>,
    
    /// Changed cells (sparse representation)
    pub cell_changes: Vec<CellChange>,
    
    /// Dimension change if any
    pub dimension_change: Option<DimensionChange>,
    
    /// Cursor movement
    pub cursor_change: Option<CursorChange>,
    
    
    /// Sequence number for ordering
    pub sequence: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CellChange {
    pub row: u16,
    pub col: u16,
    pub old_cell: Cell,
    pub new_cell: Cell,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DimensionChange {
    pub old_width: u16,
    pub old_height: u16,
    pub new_width: u16,
    pub new_height: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CursorChange {
    pub old_position: CursorPosition,
    pub new_position: CursorPosition,
}

impl GridDelta {
    /// Create minimal delta between two grids
    pub fn diff(old: &Grid, new: &Grid) -> Self {
        let mut cell_changes = Vec::new();
        
        // Only track actual changes
        for row in 0..old.height.min(new.height) {
            for col in 0..old.width.min(new.width) {
                let old_cell = old.get_cell(row, col);
                let new_cell = new.get_cell(row, col);
                
                if old_cell != new_cell {
                    if let (Some(old), Some(new)) = (old_cell, new_cell) {
                        cell_changes.push(CellChange {
                            row,
                            col,
                            old_cell: old.clone(),
                            new_cell: new.clone(),
                        });
                    }
                }
            }
        }
        
        // Check dimension changes
        let dimension_change = if old.width != new.width || old.height != new.height {
            Some(DimensionChange {
                old_width: old.width,
                old_height: old.height,
                new_width: new.width,
                new_height: new.height,
            })
        } else {
            None
        };
        
        // Check cursor changes
        let cursor_change = if old.cursor != new.cursor {
            Some(CursorChange {
                old_position: old.cursor.clone(),
                new_position: new.cursor.clone(),
            })
        } else {
            None
        };
        
        GridDelta {
            timestamp: new.timestamp,
            cell_changes,
            dimension_change,
            cursor_change,
            scrollback_additions: Vec::new(), // Calculated separately
            sequence: 0, // Set by history manager
        }
    }
    
    /// Apply delta to grid
    pub fn apply(&self, grid: &mut Grid) -> Result<(), ApplyError> {
        // Apply dimension changes first
        if let Some(dim_change) = &self.dimension_change {
            grid.resize(dim_change.new_width, dim_change.new_height)?;
        }
        
        // Apply cell changes
        for change in &self.cell_changes {
            grid.set_cell(change.row, change.col, change.new_cell.clone());
        }
        
        // Apply cursor changes
        if let Some(cursor_change) = &self.cursor_change {
            grid.cursor = cursor_change.new_position.clone();
        }
        
        
        Ok(())
    }
    
    /// Reverse a delta (for undo functionality)
    pub fn reverse(&self) -> Self {
        GridDelta {
            timestamp: self.timestamp,
            cell_changes: self.cell_changes.iter().map(|c| CellChange {
                row: c.row,
                col: c.col,
                old_cell: c.new_cell.clone(),
                new_cell: c.old_cell.clone(),
            }).collect(),
            dimension_change: self.dimension_change.as_ref().map(|d| DimensionChange {
                old_width: d.new_width,
                old_height: d.new_height,
                new_width: d.old_width,
                new_height: d.old_height,
            }),
            cursor_change: self.cursor_change.as_ref().map(|c| CursorChange {
                old_position: c.new_position.clone(),
                new_position: c.old_position.clone(),
            }),
            sequence: self.sequence,
        }
    }
}
```

### 6. Grid History (grid_history.rs)

Manages the sequence of grids and deltas with efficient lookup mechanisms.

```rust
use chrono::{DateTime, Utc, Duration};
use std::collections::BTreeMap;
use serde::{Serialize, Deserialize};

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
        
        // Check if we should coalesce with previous delta
        if self.should_coalesce(&delta) {
            self.coalesce_delta(delta);
        } else {
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
        }
        
        // Check memory limits
        self.enforce_memory_limits();
    }
    
    /// Lookup grid at specific line number
    pub fn lookup_by_line(&self, line: &LineCounter) -> Result<Grid, LookupError> {
        // Find the nearest snapshot before this line
        let snapshot_seq = self.line_index
            .range(..=line)
            .rev()
            .next()
            .map(|(_, seq)| *seq)
            .unwrap_or(0);
        
        // Reconstruct from snapshot
        self.reconstruct_from_sequence(snapshot_seq)
    }
    
    /// Lookup grid at specific time
    pub fn lookup_by_time(&self, time: DateTime<Utc>) -> Result<Grid, LookupError> {
        // Find the nearest change before this time
        let sequence = self.time_index
            .range(..=time)
            .rev()
            .next()
            .map(|(_, seq)| *seq)
            .unwrap_or(0);
        
        self.reconstruct_from_sequence(sequence)
    }
    
    /// Lookup grid at relative time from start
    pub fn lookup_by_relative_time(&self, duration: Duration) -> Result<Grid, LookupError> {
        let target_time = self.initial_grid.timestamp + duration;
        self.lookup_by_time(target_time)
    }
    
    /// Get current grid state
    pub fn get_current(&self) -> Result<Grid, LookupError> {
        self.reconstruct_from_sequence(self.current_sequence)
    }
    
    /// Reconstruct grid from nearest snapshot
    fn reconstruct_from_sequence(&self, target_seq: u64) -> Result<Grid, LookupError> {
        // Find nearest snapshot at or before target
        let (snapshot_seq, mut grid) = self.snapshots
            .range(..=target_seq)
            .rev()
            .next()
            .map(|(seq, grid)| (*seq, grid.clone()))
            .ok_or(LookupError::NoSnapshot)?;
        
        // Apply deltas from snapshot to target
        for (_, delta) in self.deltas.range(snapshot_seq + 1..=target_seq) {
            delta.apply(&mut grid)?;
        }
        
        Ok(grid)
    }
    
    /// Create a new snapshot at current state
    fn create_snapshot(&mut self) {
        if let Ok(grid) = self.get_current() {
            // Compress if enabled
            let grid = if self.config.enable_compression {
                // Store compressed version
                grid // TODO: Implement compression
            } else {
                grid
            };
            
            self.snapshots.insert(self.current_sequence, grid.clone());
            self.line_index.insert(grid.start_line.clone(), self.current_sequence);
        }
    }
    
    /// Check if delta should be coalesced with previous
    fn should_coalesce(&self, delta: &GridDelta) -> bool {
        if let Some((_, last_delta)) = self.deltas.iter().rev().next() {
            let time_diff = delta.timestamp - last_delta.timestamp;
            time_diff.num_milliseconds() < self.config.coalesce_window_ms as i64
        } else {
            false
        }
    }
    
    /// Coalesce delta with previous delta
    fn coalesce_delta(&mut self, delta: GridDelta) {
        if let Some((seq, last_delta)) = self.deltas.iter_mut().rev().next() {
            // Merge cell changes
            for change in delta.cell_changes {
                // Check if we already have a change for this cell
                if let Some(existing) = last_delta.cell_changes.iter_mut()
                    .find(|c| c.row == change.row && c.col == change.col) {
                    // Update the new_cell, keep original old_cell
                    existing.new_cell = change.new_cell;
                } else {
                    last_delta.cell_changes.push(change);
                }
            }
            
            // Update dimension change to latest
            if delta.dimension_change.is_some() {
                last_delta.dimension_change = delta.dimension_change;
            }
            
            // Update cursor to latest position
            if delta.cursor_change.is_some() {
                last_delta.cursor_change = delta.cursor_change;
            }
            
            // Update timestamp to latest
            last_delta.timestamp = delta.timestamp;
        }
    }
    
    /// Enforce memory limits using a modified ring buffer approach
    fn enforce_memory_limits(&mut self) {
        // Calculate approximate memory usage
        let estimated_size = self.estimate_memory_usage();
        
        if estimated_size > self.config.max_size_bytes {
            // When we exceed the limit, recalculate the initial grid to be the current state
            // This effectively "resets" our history while maintaining continuity
            
            // Get the current grid state
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
}
```

### 7. Grid View (grid_view.rs)

Derives viewable grids from history with re-wrapping support.

```rust
use anyhow::Result;
use unicode_width::UnicodeWidthStr;

pub struct GridView {
    history: Arc<Mutex<GridHistory>>,
}

impl GridView {
    pub fn new(history: Arc<Mutex<GridHistory>>) -> Self {
        GridView { history }
    }
    
    /// Derive realtime view with optional dimensions
    pub fn derive_realtime(&self, dimensions: Option<(u16, u16)>) -> Result<Grid> {
        let history = self.history.lock().unwrap();
        let mut grid = history.get_current()?;
        
        if let Some((width, height)) = dimensions {
            grid = self.rewrap_grid(grid, width, height)?;
        }
        
        Ok(grid)
    }
    
    /// Derive view from specific start line
    pub fn derive_from_line(
        &self, 
        start_line: &LineCounter, 
        dimensions: Option<(u16, u16)>
    ) -> Result<Grid> {
        let history = self.history.lock().unwrap();
        let mut grid = history.lookup_by_line(start_line)?;
        
        if let Some((width, height)) = dimensions {
            grid = self.rewrap_grid(grid, width, height)?;
        }
        
        Ok(grid)
    }
    
    /// Derive view at specific time
    pub fn derive_at_time(
        &self, 
        time: DateTime<Utc>, 
        dimensions: Option<(u16, u16)>
    ) -> Result<Grid> {
        let history = self.history.lock().unwrap();
        let mut grid = history.lookup_by_time(time)?;
        
        if let Some((width, height)) = dimensions {
            grid = self.rewrap_grid(grid, width, height)?;
        }
        
        Ok(grid)
    }
    
    /// Derive view at relative time
    pub fn derive_at_relative_time(
        &self, 
        duration: Duration, 
        dimensions: Option<(u16, u16)>
    ) -> Result<Grid> {
        let history = self.history.lock().unwrap();
        let mut grid = history.lookup_by_relative_time(duration)?;
        
        if let Some((width, height)) = dimensions {
            grid = self.rewrap_grid(grid, width, height)?;
        }
        
        Ok(grid)
    }
    
    /// Re-wrap grid content to new dimensions
    fn rewrap_grid(&self, grid: Grid, new_width: u16, new_height: u16) -> Result<Grid> {
        let mut new_grid = Grid::new(new_width, new_height);
        new_grid.timestamp = grid.timestamp;
        new_grid.cursor = grid.cursor.clone();
        
        // Convert grid to continuous text stream
        let text_stream = self.grid_to_text_stream(&grid);
        
        // Re-wrap text stream to new dimensions
        let wrapped_lines = self.wrap_text_stream(&text_stream, new_width);
        
        // Fill new grid with wrapped content
        // Note: Lines exceeding viewport height are simply not included in the new grid
        // They remain accessible through history lookup
        for (row_idx, line) in wrapped_lines.iter().enumerate() {
            if row_idx < new_height as usize {
                for (col_idx, cell) in line.iter().enumerate() {
                    if col_idx < new_width as usize {
                        new_grid.set_cell(row_idx as u16, col_idx as u16, cell.clone());
                    }
                }
            }
        }
        
        // Recalculate line counters
        new_grid.start_line = grid.start_line.clone();
        let total_lines = wrapped_lines.len() as u64;
        new_grid.end_line = LineCounter::from_u64(
            grid.start_line.value.to_u64().unwrap_or(0) + total_lines - 1
        );
        
        Ok(new_grid)
    }
    
    /// Convert grid to continuous text stream preserving attributes
    fn grid_to_text_stream(&self, grid: &Grid) -> Vec<Cell> {
        let mut stream = Vec::new();
        
        // Add visible grid
        for row in &grid.cells {
            // Find last non-space cell in row
            let mut last_non_space = 0;
            for (idx, cell) in row.iter().enumerate().rev() {
                if cell.char != ' ' {
                    last_non_space = idx;
                    break;
                }
            }
            
            // Add cells up to last non-space
            for cell in &row[..=last_non_space] {
                stream.push(cell.clone());
            }
            
            // Add newline marker
            stream.push(Cell {
                char: '\n',
                fg_color: Color::Default,
                bg_color: Color::Default,
                attributes: CellAttributes::default(),
            });
        }
        
        stream
    }
    
    /// Wrap text stream to specified width
    fn wrap_text_stream(&self, stream: &[Cell], width: u16) -> Vec<Vec<Cell>> {
        let mut wrapped = Vec::new();
        let mut current_line = Vec::new();
        let mut current_width = 0u16;
        
        for cell in stream {
            if cell.char == '\n' {
                // End of line
                wrapped.push(current_line.clone());
                current_line.clear();
                current_width = 0;
            } else {
                // Calculate character width
                let char_width = UnicodeWidthStr::width(cell.char.to_string().as_str()) as u16;
                
                if current_width + char_width > width {
                    // Need to wrap
                    wrapped.push(current_line.clone());
                    current_line.clear();
                    current_width = 0;
                }
                
                current_line.push(cell.clone());
                current_width += char_width;
            }
        }
        
        // Add any remaining content
        if !current_line.is_empty() {
            wrapped.push(current_line);
        }
        
        wrapped
    }
}
```

## Integration Points

### 1. PTY Reader Integration

Modify `io.rs` to capture terminal output:

```rust
// In spawn_pty_reader function
pub fn spawn_pty_reader(
    master_reader: Arc<Mutex<Option<Box<dyn std::io::Read + Send>>>>,
    state_tracker: Arc<Mutex<TerminalStateTracker>>, // NEW
) -> JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        let mut buffer = [0u8; 4096];
        loop {
            let read_result = {
                // ... existing read logic ...
            };
            
            match read_result {
                Some(Ok(data)) => {
                    // Update terminal state
                    state_tracker.lock().unwrap().process_output(&data); // NEW
                    
                    // ... existing output logic ...
                }
                // ... rest of existing logic ...
            }
        }
    })
}
```

### 2. Terminal State Tracker

New component to coordinate state updates:

```rust
pub struct TerminalStateTracker {
    current_grid: Grid,
    history: Arc<Mutex<GridHistory>>,
    parser: AnsiParser,
    last_update: DateTime<Utc>,
    update_interval: Duration,
}

impl TerminalStateTracker {
    pub fn new(width: u16, height: u16) -> Self {
        let initial_grid = Grid::new(width, height);
        let history = Arc::new(Mutex::new(GridHistory::new(initial_grid.clone())));
        
        TerminalStateTracker {
            current_grid: initial_grid,
            history,
            parser: AnsiParser::new(),
            last_update: Utc::now(),
            update_interval: Duration::milliseconds(50),
        }
    }
    
    pub fn process_output(&mut self, data: &[u8]) {
        // Parse ANSI sequences and update grid
        let changes = self.parser.parse(data);
        
        for change in changes {
            match change {
                AnsiChange::Text(text) => self.add_text(text),
                AnsiChange::CursorMove(row, col) => self.move_cursor(row, col),
                AnsiChange::Clear => self.clear_screen(),
                AnsiChange::Color(fg, bg) => self.set_colors(fg, bg),
                // ... handle other ANSI sequences
            }
        }
        
        // Create delta if enough time has passed
        let now = Utc::now();
        if now - self.last_update > self.update_interval {
            self.create_delta();
            self.last_update = now;
        }
    }
    
    fn create_delta(&mut self) {
        let mut history = self.history.lock().unwrap();
        let previous = history.get_current().unwrap_or(self.current_grid.clone());
        let delta = GridDelta::diff(&previous, &self.current_grid);
        history.add_delta(delta);
    }
}
```

## Memory Management Strategy

### 1. Compression Techniques
- **RLE Encoding**: For runs of identical cells
- **Dictionary Compression**: For common patterns
- **Zstd Compression**: For snapshots
- **Delta Chains**: Limit chain length with periodic snapshots

### 2. Memory Limits
- **Ring Buffer**: Drop oldest history when limit reached
- **Tiered Storage**: Move old snapshots to disk
- **Adaptive Snapshots**: More snapshots during high activity
- **Lazy Loading**: Load history on-demand

### 3. Performance Optimizations
- **Incremental Updates**: Only process changed regions
- **Batch Processing**: Coalesce rapid changes
- **Background Compression**: Compress snapshots asynchronously
- **Index Caching**: Cache frequently accessed lookups

## Testing Strategy

### Unit Tests
- Cell serialization/deserialization
- Grid delta generation and application
- Re-wrapping algorithms
- Index lookups
- Memory limit enforcement

### Integration Tests
- Full terminal session recording
- Playback at different dimensions
- Time-travel navigation
- Multi-client synchronization
- Performance benchmarks

### Property-Based Tests
- Delta reversibility
- Re-wrap consistency
- Index accuracy
- Compression ratio

## Performance Benchmarks

### Target Metrics
- **Delta Creation**: < 1ms
- **Snapshot Lookup**: < 10ms
- **Re-wrapping**: < 50ms for typical terminal
- **Memory Growth**: < 10MB/hour for typical usage
- **Compression Ratio**: > 10:1 for snapshots

## Future Enhancements

### Phase 2
- Disk persistence
- Network streaming
- Search functionality
- Selective history export

### Phase 3
- Machine learning for pattern detection
- Predictive pre-fetching
- Distributed storage
- Real-time collaboration features

## Error Handling

All operations should use Result types with specific error variants:

```rust
#[derive(Debug, thiserror::Error)]
pub enum TerminalStateError {
    #[error("Grid lookup failed: {0}")]
    LookupError(String),
    
    #[error("Delta application failed: {0}")]
    ApplyError(String),
    
    #[error("Serialization error: {0}")]
    SerializationError(String),
    
    #[error("Memory limit exceeded")]
    MemoryLimitExceeded,
    
    #[error("Invalid dimensions: {width}x{height}")]
    InvalidDimensions { width: u16, height: u16 },
}
```

## Configuration

Expose configuration through environment variables and config file:

```rust
pub struct TerminalStateConfig {
    pub max_memory_mb: usize,
    pub snapshot_interval: u64,
    pub compression_enabled: bool,
    pub coalesce_window_ms: u64,
    pub scrollback_limit: usize,
    pub persist_to_disk: bool,
    pub disk_path: PathBuf,
}

impl Default for TerminalStateConfig {
    fn default() -> Self {
        TerminalStateConfig {
            max_memory_mb: 100,
            snapshot_interval: 1000,
            compression_enabled: true,
            coalesce_window_ms: 50,
            scrollback_limit: 10000,
            persist_to_disk: false,
            disk_path: PathBuf::from("/tmp/beach_state"),
        }
    }
}
```

## Implementation Priority

1. **Core Data Structures** (Week 1)
   - Cell, Grid, GridDelta
   - Basic serialization

2. **History Management** (Week 2)
   - GridHistory with basic operations
   - Memory management

3. **Re-wrapping Logic** (Week 3)
   - GridView implementation
   - Dimension handling

4. **Integration** (Week 4)
   - PTY integration
   - ANSI parsing
   - Testing

5. **Optimization** (Week 5+)
   - Compression
   - Performance tuning
   - Advanced features
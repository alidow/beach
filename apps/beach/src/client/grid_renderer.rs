/// Grid renderer for terminal client - UNIFIED GRID ARCHITECTURE
/// Maintains a single unified grid sized to match the full server terminal history
use crate::server::terminal_state::{Cell, CellAttributes, Color, Grid, GridDelta};
use crate::subscription::HistoryMetadata;
use crossterm::terminal;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color as RatatuiColor, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph},
};
use std::collections::{BTreeMap, HashSet};
use std::io;

/// Historical line data for cache (DEPRECATED - kept for compatibility)
#[derive(Clone, Debug)]
pub struct HistoryLine {
    /// The actual cell content
    pub cells: Vec<Cell>,
    /// Absolute line number in terminal history
    pub line_number: u64,
}

/// Request for historical lines (DEPRECATED - no longer needed with unified grid)
#[derive(Clone, Debug)]
pub struct HistoryRequest {
    /// Start line number (inclusive)
    pub start_line: u64,
    /// End line number (inclusive)
    pub end_line: u64,
}

/// Manages the unified terminal grid and rendering
pub struct GridRenderer {
    /// Full-size grid matching server history - THE SINGLE SOURCE OF TRUTH
    /// Each row is Option<Vec<Cell>> to track loaded vs unloaded content
    pub unified_grid: Vec<Option<Vec<Cell>>>,

    /// Server dimensions from HistoryMetadata
    server_total_lines: u64,
    pub server_width: u16,
    server_visible_height: u16,

    /// Line mapping - maps grid index to absolute line number
    line_offset: u64, // Maps grid index to absolute line number

    /// Client-side scroll state (pure local)
    scroll_position: u64, // Current top line being displayed

    /// Local terminal dimensions
    local_width: u16,
    local_height: u16,

    /// DEPRECATED: Legacy fields kept for compatibility during transition
    /// These will be removed once the unified grid is fully operational
    pub server_height: u16, // Legacy: kept for backward compatibility
    pub grid: Grid, // Legacy: kept for backward compatibility
    history_cache: BTreeMap<u64, HistoryLine>,
    pending_requests: HashSet<String>,
    pub history_metadata: Option<HistoryMetadata>,
    view_line: Option<u64>,
    pub scroll_offset: u16,
    horizontal_offset: u16,
    overscan_buffer: Vec<Grid>,
    from_line: u64,
    last_snapshot: Option<Grid>,

    /// Whether to show the debug size status line
    pub debug_size: bool,
}

impl GridRenderer {
    /// Create a new grid renderer (LEGACY - for viewport-based mode)
    pub fn new(server_width: u16, server_height: u16, debug_size: bool) -> io::Result<Self> {
        let (local_width, local_height) = terminal::size()?;

        let grid = Grid::new(server_width, server_height);

        Ok(Self {
            // Legacy fields
            server_height,
            grid,
            history_cache: BTreeMap::new(),
            pending_requests: HashSet::new(),
            history_metadata: None,
            view_line: None,
            scroll_offset: 0,
            horizontal_offset: 0,
            overscan_buffer: Vec::new(),
            from_line: 0,
            last_snapshot: None,
            debug_size,
            local_width,
            local_height,

            // NEW: Unified grid fields (initialized as empty)
            unified_grid: Vec::new(),
            server_total_lines: 0,
            server_width,
            server_visible_height: server_height,
            line_offset: 0,
            scroll_position: 0,
        })
    }

    /// Create a new unified grid renderer based on server HistoryMetadata message (NEW)
    pub fn new_with_unified_metadata(
        total_lines: u64,
        oldest_line: u64,
        latest_line: u64,
        terminal_width: u16,
        terminal_height: u16,
        debug_size: bool,
    ) -> io::Result<Self> {
        let (local_width, local_height) = terminal::size()?;

        // Initialize unified grid with full history size
        let total_rows = total_lines as usize;
        let mut unified_grid = Vec::with_capacity(total_rows);

        // Initialize all rows as None (not yet loaded)
        for _ in 0..total_rows {
            unified_grid.push(None);
        }

        // Calculate initial scroll position (show most recent content)
        let scroll_position = latest_line.saturating_sub(terminal_height as u64);

        Ok(Self {
            // NEW: Unified grid fields
            unified_grid,
            server_total_lines: total_lines,
            server_width: terminal_width,
            server_visible_height: terminal_height,
            line_offset: oldest_line,
            scroll_position,
            local_width,
            local_height,
            debug_size,

            // Legacy fields (for compatibility)
            server_height: terminal_height,
            grid: Grid::new(terminal_width, terminal_height),
            history_cache: BTreeMap::new(),
            pending_requests: HashSet::new(),
            history_metadata: None, // Will be set later if needed
            view_line: None,
            scroll_offset: 0,
            horizontal_offset: 0,
            overscan_buffer: Vec::new(),
            from_line: 0,
            last_snapshot: None,
        })
    }

    /// Update local terminal dimensions
    pub fn resize_local(&mut self, width: u16, height: u16) {
        self.local_width = width;
        self.local_height = height;
    }

    /// Initialize unified grid with full history size
    pub fn initialize_unified_grid(
        &mut self,
        total_lines: u64,
        oldest_line: u64,
        latest_line: u64,
        terminal_width: u16,
        terminal_height: u16,
    ) {
        // Initialize grid with full history size
        let total_rows = total_lines as usize;
        self.unified_grid = Vec::with_capacity(total_rows);

        // Initialize all rows as None (not yet loaded)
        for _ in 0..total_rows {
            self.unified_grid.push(None);
        }

        // Store server dimensions
        self.server_total_lines = total_lines;
        self.server_width = terminal_width;
        self.line_offset = oldest_line;

        // Set scroll position to show the latest content
        self.scroll_position = latest_line.saturating_sub(terminal_height as u64);
    }

    /// NEW: Apply initial snapshot to unified grid
    pub fn apply_initial_snapshot_unified(&mut self, snapshot: Grid) {
        if self.unified_grid.is_empty() {
            return; // Not in unified grid mode
        }

        // Map snapshot lines to grid positions
        if let Some(start_line) = snapshot.start_line.to_u64() {
            let start_idx = start_line.saturating_sub(self.line_offset) as usize;

            for (i, row) in snapshot.cells.iter().enumerate() {
                let grid_idx = start_idx + i;
                if grid_idx < self.unified_grid.len() {
                    self.unified_grid[grid_idx] = Some(row.clone());
                }
            }
        }
    }

    /// NEW: Apply history chunk to unified grid
    pub fn apply_history_chunk_unified(
        &mut self,
        start_line: u64,
        end_line: u64,
        rows: Vec<Vec<Cell>>,
    ) {
        if self.unified_grid.is_empty() {
            return; // Not in unified grid mode
        }

        // Map chunk lines to grid positions
        let start_idx = start_line.saturating_sub(self.line_offset) as usize;

        for (i, row) in rows.iter().enumerate() {
            let grid_idx = start_idx + i;
            if grid_idx < self.unified_grid.len() {
                self.unified_grid[grid_idx] = Some(row.clone());
            }
        }
    }

    /// NEW: Apply delta to unified grid
    pub fn apply_delta_unified(&mut self, delta: &GridDelta) {
        if self.unified_grid.is_empty() {
            return; // Not in unified grid mode
        }

        // Apply delta to the appropriate rows
        // Deltas always apply to recent rows (near the end of grid)
        for change in &delta.cell_changes {
            let row_idx = (change.row as u64 + self.server_total_lines
                - self.server_visible_height as u64) as usize;
            if row_idx < self.unified_grid.len() {
                // Ensure row exists and has data
                if self.unified_grid[row_idx].is_none() {
                    self.unified_grid[row_idx] =
                        Some(vec![Cell::default(); self.server_width as usize]);
                }
                if let Some(ref mut row) = self.unified_grid[row_idx] {
                    if (change.col as usize) < row.len() {
                        row[change.col as usize] = change.new_cell.clone();
                    }
                }
            }
        }

        // Handle new lines being added (scrolling)
        if let Some(dim_change) = &delta.dimension_change {
            if dim_change.new_height > dim_change.old_height {
                let lines_added = dim_change.new_height - dim_change.old_height;
                // Add new rows at the end
                for _ in 0..lines_added {
                    self.unified_grid
                        .push(Some(vec![Cell::default(); self.server_width as usize]));
                    self.server_total_lines += 1;
                }
            }
        }
    }

    /// NEW: Client-side scrolling for unified grid
    pub fn scroll_unified(&mut self, delta: i64) {
        if self.unified_grid.is_empty() {
            return; // Not in unified grid mode
        }

        // Simple client-side scrolling
        let new_position = (self.scroll_position as i64 + delta)
            .max(0)
            .min((self.server_total_lines - self.server_visible_height as u64) as i64)
            as u64;
        self.scroll_position = new_position;
    }

    /// Apply a snapshot from the server
    pub fn apply_snapshot(&mut self, snapshot: Grid) {
        let old_scroll_offset = self.scroll_offset;
        let old_view_line = self.view_line;

        // Debug logging at start of apply_snapshot
        if let Ok(mut debug_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/beach-debug.log")
        {
            use std::io::Write;
            let _ = writeln!(
                debug_file,
                "[{}] [APPLY_SNAPSHOT] BEFORE: scroll_offset={}, view_line={:?}, snapshot.start_line={:?}, is_historical={}",
                chrono::Utc::now().format("%H:%M:%S%.3f"),
                old_scroll_offset,
                old_view_line,
                snapshot.start_line.to_u64(),
                self.view_line.is_some()
            );
        }

        // Update server dimensions from snapshot
        self.server_width = snapshot.width;
        self.server_height = snapshot.height;

        // Check if this is a historical snapshot
        let is_historical = self.view_line.is_some();

        // If this is a historical snapshot, add lines to cache
        if is_historical {
            // Extract lines from the snapshot and add to cache
            let start_line = snapshot.start_line.to_u64().unwrap_or(0);
            for (i, row) in snapshot.cells.iter().enumerate() {
                let line_num = start_line + i as u64;
                let history_line = HistoryLine {
                    cells: row.clone(),
                    line_number: line_num,
                };
                self.history_cache.insert(line_num, history_line);
            }
            // Update our historical anchor to the snapshot's start
            if let Some(s) = snapshot.start_line.to_u64() {
                self.view_line = Some(s);
                // FIXED: Do NOT reset scroll_offset - preserve user's scroll position!
            }
        }

        // FIXED: Merge snapshot data instead of replacing entire grid
        self.merge_snapshot_into_grid(snapshot.clone());
        self.last_snapshot = Some(snapshot);

        // Only reset scroll offset if we're in realtime mode AND not currently scrolled
        // This preserves user's attempt to scroll while waiting for history.
        if !is_historical && self.scroll_offset == 0 {
            self.scroll_offset = 0;
        }

        // Debug logging after apply_snapshot
        if let Ok(mut debug_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/beach-debug.log")
        {
            use std::io::Write;
            let _ = writeln!(
                debug_file,
                "[{}] [APPLY_SNAPSHOT] AFTER: scroll_offset={}, view_line={:?} (PRESERVED - scroll_offset {} → {})",
                chrono::Utc::now().format("%H:%M:%S%.3f"),
                self.scroll_offset,
                self.view_line,
                old_scroll_offset,
                self.scroll_offset
            );
        }
    }

    /// Apply a delta to the current grid
    pub fn apply_delta(&mut self, delta: &GridDelta) {
        // Apply cell changes
        for change in &delta.cell_changes {
            let row_idx = change.row as usize;
            let col_idx = change.col as usize;

            if row_idx < self.grid.cells.len() {
                if col_idx < self.grid.cells[row_idx].len() {
                    self.grid.cells[row_idx][col_idx] = change.new_cell.clone();
                }
            }
        }

        // Apply cursor position changes
        if let Some(cursor_change) = &delta.cursor_change {
            self.grid.cursor = cursor_change.new_position.clone();
        }

        // Handle dimension changes
        if let Some(dim_change) = &delta.dimension_change {
            self.server_width = dim_change.new_width;
            self.server_height = dim_change.new_height;

            // Resize the grid if dimensions changed
            let new_width = dim_change.new_width as usize;
            let new_height = dim_change.new_height as usize;

            // Resize height (add/remove rows)
            self.grid
                .cells
                .resize(new_height, vec![Cell::default(); new_width]);

            // Resize width of each row
            for row in &mut self.grid.cells {
                row.resize(new_width, Cell::default());
            }

            // Update grid dimensions
            self.grid.width = dim_change.new_width;
            self.grid.height = dim_change.new_height;
        }
    }

    /// Scroll vertically
    pub fn scroll_vertical(&mut self, delta: i16) {
        // Check if we're in unified grid mode
        if !self.unified_grid.is_empty() {
            // Use unified grid scrolling
            self.scroll_unified(delta as i64);
            return;
        }

        // Legacy viewport-based scrolling
        // Delta convention:
        // Positive delta = scroll content up (view earlier/older content)
        // Negative delta = scroll content down (view later/newer content)

        let old_scroll_offset = self.scroll_offset;

        // If we have history metadata, use it to determine scroll bounds
        if let Some(metadata) = &self.history_metadata {
            // We can scroll through the entire history
            let current_line = self.grid.end_line.to_u64().unwrap_or(metadata.latest_line);

            if delta > 0 {
                // Scrolling up (viewing older content)
                // Increase scroll offset to show earlier lines
                self.scroll_offset = self.scroll_offset.saturating_add(delta as u16);

                // Calculate target line
                let target_line = current_line.saturating_sub(self.scroll_offset as u64);

                // Don't scroll past the oldest line
                if target_line < metadata.oldest_line {
                    let max_offset = (current_line - metadata.oldest_line) as u16;
                    self.scroll_offset = max_offset;
                }
            } else {
                // Scrolling down (viewing newer content)
                // Decrease scroll offset
                self.scroll_offset = self.scroll_offset.saturating_sub(delta.abs() as u16);
            }
        } else {
            // No history metadata, fall back to grid-based scrolling
            let visible_height = (self.local_height - 1) as usize;
            let grid_height = self.grid.cells.len();
            let max_scroll = grid_height.saturating_sub(visible_height) as u16;

            let new_offset = if delta > 0 {
                self.scroll_offset
                    .saturating_add(delta as u16)
                    .min(max_scroll)
            } else {
                self.scroll_offset.saturating_sub(delta.abs() as u16)
            };

            self.scroll_offset = new_offset;
        }

        // Debug logging: Track scroll_offset changes from user scrolling
        if self.scroll_offset != old_scroll_offset {
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/beach-debug.log")
            {
                use std::io::Write;
                let _ = writeln!(
                    file,
                    "[{}] [SCROLL_VERTICAL] User scroll delta={} changed scroll_offset: {} → {}",
                    chrono::Utc::now().format("%H:%M:%S%.3f"),
                    delta,
                    old_scroll_offset,
                    self.scroll_offset
                );
            }
        }
    }

    /// Scroll horizontally (when local width < server width)
    pub fn scroll_horizontal(&mut self, delta: i16) {
        if self.local_width < self.server_width {
            let max_offset = self.server_width.saturating_sub(self.local_width);
            let new_offset = (self.horizontal_offset as i16 + delta).max(0) as u16;
            self.horizontal_offset = new_offset.min(max_offset);
        }
    }

    /// Check if horizontal scrolling is needed
    pub fn needs_horizontal_scroll(&self) -> bool {
        self.local_width < self.server_width
    }

    /// Get the subscription parameters for overscan
    pub fn get_overscan_params(&self) -> (u64, u16) {
        // Request 2x the visible height for smooth scrolling
        let height = self.local_height * 2;
        (self.from_line, height)
    }

    /// Update the from_line based on scroll position
    pub fn update_from_line(&mut self, new_from_line: u64) {
        self.from_line = new_from_line;
    }

    /// Update history metadata from server
    pub fn set_history_metadata(&mut self, metadata: HistoryMetadata) {
        self.history_metadata = Some(metadata);
    }

    /// Add historical lines to cache
    pub fn add_history_lines(&mut self, lines: Vec<HistoryLine>) {
        for line in lines {
            self.history_cache.insert(line.line_number, line);
        }
    }

    /// Calculate what history needs to be fetched for current scroll position
    pub fn calculate_history_needs(&self) -> Option<HistoryRequest> {
        // Debug logging to file
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/calculate-history-debug.log")
        {
            use std::io::Write;
            let _ = writeln!(
                file,
                "[{}] calculate_history_needs: scroll_offset={}, view_line={:?}, has_metadata={}, grid_height={}, local_height={}",
                chrono::Utc::now(),
                self.scroll_offset,
                self.view_line,
                self.history_metadata.is_some(),
                self.grid.height,
                self.local_height
            );
        }

        // Metadata may be unavailable initially; allow fallback when scrolling
        let metadata = self.history_metadata.as_ref();

        // If in realtime mode and at bottom, no history needed
        if self.view_line.is_none() && self.scroll_offset == 0 {
            return None;
        }

        // If we have a scroll offset but NOT in historical mode yet, request to enter history
        if self.view_line.is_none() && self.scroll_offset > 0 {
            // Calculate the target line we want to view
            let end_line_u64 = self.grid.end_line.to_u64();

            // Choose an effective end line using metadata if available
            let effective_end_line = if let Some(meta) = metadata {
                end_line_u64.unwrap_or(meta.latest_line)
            } else {
                end_line_u64.unwrap_or_else(|| self.grid.height.saturating_sub(1) as u64)
            };
            // Adjust for trailing blank rows at the bottom of the current grid
            let mut trailing_blanks = 0u64;
            for row in (0..self.grid.height).rev() {
                let is_blank = (0..self.grid.width).all(|col| {
                    self.grid
                        .get_cell(row, col)
                        .map(|c| c.char == ' ' || c.char == '\0' || c.char == '\u{00A0}')
                        .unwrap_or(true)
                });
                if is_blank {
                    trailing_blanks += 1;
                } else {
                    break;
                }
            }
            let effective_end_line_adj = effective_end_line.saturating_sub(trailing_blanks);
            let current_top_line =
                effective_end_line_adj.saturating_sub(self.local_height as u64 - 1); // Approximate top line
            let target_line = current_top_line.saturating_sub(self.scroll_offset as u64);

            // Clamp if we know the history range
            let mut start_line = if let Some(meta) = metadata {
                target_line.max(meta.oldest_line).min(meta.latest_line)
            } else {
                target_line
            };

            // Fallback: if our computed start_line is at or below the last-known grid end
            // (common when client line counters are stale), bump the request far enough
            // forward to land on a newer snapshot. The server will pick the closest snapshot
            // at or before this line, so requesting beyond our current end nudges us into
            // the latest available history instead of the initial blank window.
            if self.scroll_offset > 0 {
                if let Some(grid_end) = self.grid.end_line.to_u64() {
                    if start_line <= grid_end {
                        start_line = grid_end
                            .saturating_add(self.local_height as u64)
                            .saturating_add(self.scroll_offset as u64);
                    }
                }
            }

            // Request history for the scrolled position (server primarily uses start_line)
            let end_line = if let Some(meta) = metadata {
                (start_line + self.local_height as u64 + 100).min(meta.latest_line)
            } else {
                start_line + self.local_height as u64 + 100
            };
            let end_line = end_line.max(start_line); // never regress
            let request = HistoryRequest {
                start_line,
                end_line,
            };

            // Debug log the request
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/calculate-history-debug.log")
            {
                use std::io::Write;
                let _ = writeln!(
                    file,
                    "[{}] RETURNING HISTORY REQUEST (has_meta={}): start_line={}, end_line={}, eff_end={}, trailing_blanks={}, target_line={}",
                    chrono::Utc::now(),
                    metadata.is_some(),
                    request.start_line,
                    request.end_line,
                    effective_end_line,
                    trailing_blanks,
                    target_line
                );
            }

            return Some(request);
        }

        // Historical mode: compute target line relative to current anchor
        if let Some(view_line) = self.view_line {
            // No scroll in historical mode → no request
            if self.scroll_offset == 0 {
                return None;
            }
            let current_start = self.grid.start_line.to_u64().unwrap_or(0);
            let mut target_line = view_line.saturating_sub(self.scroll_offset as u64);
            if let Some(meta) = metadata {
                target_line = target_line.max(meta.oldest_line).min(meta.latest_line);
            }
            // If target is within current snapshot window, no need to fetch
            if target_line >= current_start {
                return None;
            }
            let mut end_line = if let Some(meta) = metadata {
                (target_line + self.local_height as u64 + 100).min(meta.latest_line)
            } else {
                target_line + self.local_height as u64 + 100
            };
            end_line = end_line.max(target_line);
            return Some(HistoryRequest {
                start_line: target_line,
                end_line,
            });
        }

        None
    }

    /// Switch to historical view mode at a specific line
    pub fn enter_historical_mode(&mut self, line_number: u64) {
        // Debug logging for mode transition
        if let Ok(mut debug_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/beach-debug.log")
        {
            use std::io::Write;
            let _ = writeln!(
                debug_file,
                "[{}] [ENTER_HISTORICAL_MODE] view_line: {:?} -> Some({}), preserving scroll_offset={}",
                chrono::Utc::now().format("%H:%M:%S%.3f"),
                self.view_line,
                line_number,
                self.scroll_offset
            );
        }

        self.view_line = Some(line_number);
        // FIXED: Do NOT reset scroll_offset - preserve user's scroll position
    }

    /// Return to realtime mode
    pub fn enter_realtime_mode(&mut self) {
        // Debug logging for mode transition
        if let Ok(mut debug_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/beach-state-flow.log")
        {
            use std::io::Write;
            let _ = writeln!(
                debug_file,
                "[{}] [ENTER_REALTIME_MODE] view_line: {:?} -> None",
                chrono::Utc::now(),
                self.view_line
            );
        }

        self.view_line = None;
        self.scroll_offset = 0;
    }

    /// Merge snapshot data into existing grid instead of replacing it
    fn merge_snapshot_into_grid(&mut self, snapshot: Grid) {
        let snapshot_start = snapshot.start_line.to_u64().unwrap_or(0);
        let snapshot_end = snapshot.end_line.to_u64().unwrap_or(0);

        // Debug logging
        if let Ok(mut debug_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/beach-debug.log")
        {
            use std::io::Write;
            let _ = writeln!(
                debug_file,
                "[{}] [MERGE_SNAPSHOT] Before: grid_height={}, grid_range=({:?}, {:?})",
                chrono::Utc::now().format("%H:%M:%S%.3f"),
                self.grid.cells.len(),
                self.grid.start_line.to_u64(),
                self.grid.end_line.to_u64()
            );
            let _ = writeln!(
                debug_file,
                "[{}] [MERGE_SNAPSHOT] Snapshot: height={}, range=({}, {})",
                chrono::Utc::now().format("%H:%M:%S%.3f"),
                snapshot.cells.len(),
                snapshot_start,
                snapshot_end
            );
        }

        // Calculate the full range we need to cover
        let current_start = self.grid.start_line.to_u64().unwrap_or(snapshot_start);
        let current_end = self.grid.end_line.to_u64().unwrap_or(snapshot_end);

        let new_start = current_start.min(snapshot_start);
        let new_end = current_end.max(snapshot_end);
        let new_height = (new_end - new_start + 1) as usize;

        // Create placeholder cell for missing data
        let placeholder_cell = Cell {
            char: '⏳',
            fg_color: Color::Rgb(128, 128, 128),
            bg_color: Color::Default,
            attributes: CellAttributes {
                bold: false,
                italic: false,
                underline: false,
                strikethrough: false,
                reverse: false,
                blink: false,
                dim: false,
                hidden: false,
            },
        };

        // If this is the first grid or we need to expand, create new grid
        if self.grid.cells.is_empty() || new_height != self.grid.cells.len() {
            // Create new grid with placeholders
            let mut new_cells =
                vec![vec![placeholder_cell.clone(); snapshot.width as usize]; new_height];

            // Copy existing data to new grid (if any)
            if !self.grid.cells.is_empty() {
                let old_start = self.grid.start_line.to_u64().unwrap_or(0);
                for (old_row_idx, old_row) in self.grid.cells.iter().enumerate() {
                    let line_number = old_start + old_row_idx as u64;
                    if line_number >= new_start && line_number <= new_end {
                        let new_row_idx = (line_number - new_start) as usize;
                        if new_row_idx < new_cells.len() {
                            new_cells[new_row_idx] = old_row.clone();
                        }
                    }
                }
            }

            self.grid.cells = new_cells;
            self.grid.start_line = crate::server::terminal_state::LineCounter::from_u64(new_start);
            self.grid.end_line = crate::server::terminal_state::LineCounter::from_u64(new_end);
        }

        // Now merge the snapshot data
        for (snap_row_idx, snap_row) in snapshot.cells.iter().enumerate() {
            let line_number = snapshot_start + snap_row_idx as u64;
            if line_number >= new_start && line_number <= new_end {
                let grid_row_idx = (line_number - new_start) as usize;
                if grid_row_idx < self.grid.cells.len() {
                    self.grid.cells[grid_row_idx] = snap_row.clone();
                }
            }
        }

        // Update grid metadata
        self.grid.width = snapshot.width;
        self.grid.height = self.grid.cells.len() as u16;

        // Debug logging after merge
        if let Ok(mut debug_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/beach-debug.log")
        {
            use std::io::Write;
            let _ = writeln!(
                debug_file,
                "[{}] [MERGE_SNAPSHOT] After: grid_height={}, grid_range=({:?}, {:?})",
                chrono::Utc::now().format("%H:%M:%S%.3f"),
                self.grid.cells.len(),
                self.grid.start_line.to_u64(),
                self.grid.end_line.to_u64()
            );
        }
    }

    /// Check if we're in historical view mode
    pub fn is_historical_mode(&self) -> bool {
        self.view_line.is_some()
    }

    /// Get the current historical anchor (start line) if in historical mode
    pub fn historical_anchor(&self) -> Option<u64> {
        self.view_line
    }

    /// NEW: Render the unified grid to a ratatui frame
    pub fn render_unified(&self, frame: &mut Frame) {
        if self.unified_grid.is_empty() {
            return; // Not in unified grid mode, fall back to legacy render
        }

        let area = frame.area();
        let visible_height = area.height as usize;
        let start_idx = self.scroll_position as usize;
        let end_idx = (start_idx + visible_height).min(self.unified_grid.len());

        let mut lines = Vec::new();

        for row_idx in start_idx..end_idx {
            if row_idx < self.unified_grid.len() {
                match &self.unified_grid[row_idx] {
                    Some(row) => {
                        // Render actual content
                        let mut spans = Vec::new();
                        for cell in row {
                            spans.push(Span::styled(cell.char.to_string(), cell_to_style(cell)));
                        }
                        lines.push(Line::from(spans));
                    }
                    None => {
                        // Show placeholder in first cell only
                        let mut spans = vec![Span::raw("⏳")];
                        // Fill rest with spaces
                        for _ in 1..self.server_width {
                            spans.push(Span::raw(" "));
                        }
                        lines.push(Line::from(spans));
                    }
                }
            }
        }

        // Add debug info if enabled
        if self.debug_size {
            let debug_line = format!(
                "UNIFIED: {} loaded/{} total | scroll: {} | server: {}x{}",
                self.unified_grid.iter().filter(|row| row.is_some()).count(),
                self.unified_grid.len(),
                self.scroll_position,
                self.server_width,
                self.server_total_lines
            );
            lines.push(Line::from(Span::styled(
                debug_line,
                Style::default().fg(RatatuiColor::Yellow),
            )));
        }

        let paragraph = Paragraph::new(lines);
        frame.render_widget(paragraph, area);
    }

    /// Render the grid to a ratatui frame with optional predictive underlines (LEGACY)
    pub fn render(&self, frame: &mut Frame, predictions: &[(u16, u16)]) {
        let area = frame.area();

        // Enhanced render state logging
        if let Ok(mut debug_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/beach-render-state.log")
        {
            use std::io::Write;
            let _ = writeln!(debug_file, "[{}] [RENDER] State check:", chrono::Utc::now());
            let _ = writeln!(debug_file, "  view_line: {:?}", self.view_line);
            let _ = writeln!(
                debug_file,
                "  is_historical_mode(): {}",
                self.is_historical_mode()
            );
            let _ = writeln!(debug_file, "  scroll_offset: {}", self.scroll_offset);
            let _ = writeln!(
                debug_file,
                "  history_metadata: {:?}",
                self.history_metadata
            );
            let _ = writeln!(
                debug_file,
                "  grid_dims: {}x{}, area_dims: {}x{}",
                self.grid.width, self.grid.height, area.width, area.height
            );
            let _ = writeln!(
                debug_file,
                "  grid.start_line: {:?}, grid.end_line: {:?}",
                self.grid.start_line.to_u64(),
                self.grid.end_line.to_u64()
            );
        }

        // Check if we should show the status line (only when debug flag is set)
        let show_status_line = self.debug_size;

        // Create main layout based on whether we show status line
        let chunks = if show_status_line {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(1),
                    Constraint::Length(1), // Status line
                ])
                .split(area)
        } else {
            // Use entire area for content when status line is hidden
            std::rc::Rc::from(vec![area])
        };

        // Render the terminal content with predictions
        // Pass the actual content area height so we render the right number of lines
        let (content, render_start) = self
            .render_content_with_predictions_for_area_with_offset(predictions, chunks[0].height);
        // Disable wrapping for sanity check: each grid row maps to exactly one drawn row
        let paragraph = Paragraph::new(content).block(Block::default().borders(Borders::NONE));
        frame.render_widget(paragraph, chunks[0]);

        // Render cursor if visible
        if self.grid.cursor.visible && self.scroll_offset == 0 {
            // Calculate the cursor position accounting for scrolling
            let cursor_row = self.grid.cursor.row;
            let cursor_col = self.grid.cursor.col;

            // Calculate visible range
            let visible_height = chunks[0].height as usize;

            // Check if cursor is within visible area using the actual render_start from content rendering
            if cursor_row as usize >= render_start
                && (cursor_row as usize) < render_start + visible_height
            {
                // Calculate screen position
                let screen_row = cursor_row as usize - render_start;
                let screen_col = cursor_col.saturating_sub(self.horizontal_offset);

                // Only set cursor if it's within the visible horizontal range
                if screen_col < self.local_width {
                    frame.set_cursor_position((
                        chunks[0].x + screen_col,
                        chunks[0].y + screen_row as u16,
                    ));
                }
            }
        }

        // Render scrollback indicator overlay if scrolled (regardless of debug flag)
        if self.scroll_offset > 0 {
            self.render_scrollback_overlay(frame, chunks[0]);
        }

        // Render status line only if debug flag is set
        if show_status_line && chunks.len() > 1 {
            let status = self.render_status_line();
            frame.render_widget(status, chunks[1]);
        }
    }

    /// Render the grid content as text with predictive underlines for specific area height
    fn render_content_with_predictions_for_area(
        &self,
        predictions: &[(u16, u16)],
        area_height: u16,
    ) -> Text<'static> {
        let mut lines = Vec::new();

        let render_debug_enabled = std::env::var("BEACH_RENDER_DEBUG").ok().is_some();
        // Debug output to file (gated)
        if render_debug_enabled {
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/render-debug.log")
            {
                use std::io::Write;
                let _ = writeln!(file, "=== Render Debug ===");
                let _ = writeln!(file, "area_height: {}", area_height);
                let _ = writeln!(file, "grid.cells.len(): {}", self.grid.cells.len());
                let _ = writeln!(file, "local_height: {}", self.local_height);
                let _ = writeln!(file, "scroll_offset: {}", self.scroll_offset);
            }
        }

        // Use the actual area height for visible rows
        let visible_height = area_height as usize;

        // Calculate starting row
        // IMPORTANT: Terminals anchor content at the BOTTOM, not the top
        // When scroll_offset = 0, we should show the bottom-most content
        let grid_height = self.grid.cells.len();

        // Determine start row based on content and scrolling
        // In realtime mode clamp the local scroll within the available window.
        // In historical mode, calculate which rows of the grid to show based on viewport.
        let start_row = if grid_height > visible_height {
            let bottom_start = grid_height - visible_height;
            if self.is_historical_mode() {
                // In historical mode, determine which part of the grid contains
                // the content we want to show. The server may send a larger grid
                // due to prefetch, so we need to find the right offset.
                if let (Some(grid_start), Some(view_line)) =
                    (self.grid.start_line.to_u64(), self.view_line)
                {
                    // Calculate the offset within the grid for our view_line
                    let line_offset_in_grid = view_line.saturating_sub(grid_start);
                    (line_offset_in_grid as usize).min(bottom_start)
                } else {
                    // Fallback: bottom-anchor if we can't determine the offset
                    bottom_start
                }
            } else {
                let effective_offset = (self.scroll_offset as usize).min(bottom_start);
                bottom_start.saturating_sub(effective_offset)
            }
        } else {
            0
        };

        let mut render_start = start_row;
        let mut render_end = (render_start + visible_height).min(grid_height);

        // Debug log the rendering calculations
        if let Ok(mut debug_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/beach-render-debug.log")
        {
            use std::io::Write;
            let mode = if self.is_historical_mode() {
                "historical"
            } else {
                "realtime"
            };
            let grid_start = self.grid.start_line.to_u64().unwrap_or(0);
            let grid_end = self.grid.end_line.to_u64().unwrap_or(0);
            let _ = writeln!(
                debug_file,
                "[{}] RENDER_CALC mode={} grid_dims={}x{} grid_range=({},{}) view_line={:?} scroll_offset={} -> start_row={} range=({},{})",
                chrono::Utc::now().format("%H:%M:%S%.3f"),
                mode,
                self.grid.width,
                grid_height,
                grid_start,
                grid_end,
                self.view_line,
                self.scroll_offset,
                start_row,
                render_start,
                render_end
            );

            // Sample the actual content being rendered
            let mut sample_rows = Vec::new();
            for row in render_start..(render_start + 3.min(render_end - render_start)) {
                if let Some(row_cells) = self.grid.cells.get(row) {
                    let mut line = String::new();
                    for col in 0..self.grid.width.min(80) {
                        if let Some(cell) = row_cells.get(col as usize) {
                            line.push(cell.char);
                        }
                    }
                    sample_rows.push(format!("row[{}]: '{}'", row, line.trim_end()));
                }
            }
            let _ = writeln!(
                debug_file,
                "[{}] RENDER_CONTENT: {}",
                chrono::Utc::now().format("%H:%M:%S%.3f"),
                sample_rows.join(", ")
            );
        }

        // Debug log the rendering range
        if let Ok(mut debug_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/beach-render-debug.log")
        {
            use std::io::Write;
            let _ = writeln!(
                debug_file,
                "[{}] render_range: start_row={}, render_start={}, render_end={}, grid_height={}, visible_height={}",
                chrono::Utc::now(),
                start_row,
                render_start,
                render_end,
                grid_height,
                visible_height
            );
        }

        // Reduce excess trailing blank rows at the bottom of the viewport
        // to a maximum of 1 by shifting the window up when possible.
        // This avoids showing an extra blank line visually compared to server terminals.
        if render_end > render_start {
            // Count trailing blank rows in current window
            let mut trailing_blanks = 0usize;
            'scan: for row_idx in (render_start..render_end).rev() {
                if let Some(row) = self.grid.cells.get(row_idx) {
                    let is_blank = row.iter().all(|cell| {
                        let ch = cell.char;
                        ch == ' ' || ch == '\0' || ch == '\u{00A0}'
                    });
                    if is_blank {
                        trailing_blanks += 1;
                    } else {
                        break 'scan;
                    }
                } else {
                    // Treat missing rows as blank
                    trailing_blanks += 1;
                }
            }

            if trailing_blanks > 1 {
                // Shift window up by the extra blank count (leave 1 blank at bottom)
                let extra = trailing_blanks - 1;
                let shift = extra.min(render_start);
                if shift > 0 {
                    // Safe to shift the viewport up
                    render_start -= shift;
                    render_end -= shift;
                }
            }
        }

        // If we're in historical mode, avoid showing leading blank rows when possible by
        // shifting the window down to the first non-blank line (within bounds).
        if self.is_historical_mode() && render_end > render_start {
            let mut leading_blanks = 0usize;
            for row_idx in render_start..render_end {
                if let Some(row) = self.grid.cells.get(row_idx) {
                    let is_blank = row.iter().all(|cell| {
                        let ch = cell.char;
                        ch == ' ' || ch == '\0' || ch == '\u{00A0}'
                    });
                    if is_blank {
                        leading_blanks += 1;
                    } else {
                        break;
                    }
                } else {
                    leading_blanks += 1;
                }
            }
            if leading_blanks > 0 {
                let max_shift = grid_height
                    .saturating_sub(visible_height)
                    .saturating_sub(render_start);
                let shift = leading_blanks.min(max_shift);
                if shift > 0 {
                    render_start += shift;
                    render_end = (render_start + visible_height).min(grid_height);
                }
            }
        }

        // If we're in historical mode, avoid showing leading blank rows when possible by
        // shifting the window down to the first non-blank line (within bounds).
        if self.is_historical_mode() && render_end > render_start {
            let mut leading_blanks = 0usize;
            for row_idx in render_start..render_end {
                if let Some(row) = self.grid.cells.get(row_idx) {
                    let is_blank = row.iter().all(|cell| {
                        let ch = cell.char;
                        ch == ' ' || ch == '\0' || ch == '\u{00A0}'
                    });
                    if is_blank {
                        leading_blanks += 1;
                    } else {
                        break;
                    }
                } else {
                    leading_blanks += 1;
                }
            }
            if leading_blanks > 0 {
                let bottom_start = grid_height.saturating_sub(visible_height);
                let max_shift = bottom_start.saturating_sub(render_start);
                let shift = leading_blanks.min(max_shift);
                if shift > 0 {
                    render_start += shift;
                    render_end = (render_start + visible_height).min(grid_height);
                }
            }
        }

        // More debug output (gated)
        if render_debug_enabled {
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/render-debug.log")
            {
                use std::io::Write;
                let _ = writeln!(file, "visible_height: {}", visible_height);
                let _ = writeln!(file, "start_row: {}", render_start);
                let _ = writeln!(file, "end_row: {}", render_end);
                let _ = writeln!(file, "Rendering rows {} to {}", render_start, render_end);
            }
        }

        // Detect a rule line (heavy box drawing) within the render window and log a small seam window
        // Note: This is diagnostic-only to catch potential visual double-blanks
        let mut rule_row_abs: Option<u16> = None;

        for row_idx in render_start..render_end {
            if let Some(row) = self.grid.cells.get(row_idx) {
                let mut spans = Vec::new();

                // Debug: Log first few chars of each row
                if render_debug_enabled {
                    if row_idx < 3 {
                        if let Ok(mut file) = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open("/tmp/render-debug.log")
                        {
                            use std::io::Write;
                            let row_text: String = row.iter().take(40).map(|c| c.char).collect();
                            let _ = writeln!(file, "Row {}: '{}'", row_idx, row_text.trim_end());
                        }
                    }
                }

                // Handle horizontal scrolling
                let start_col = self.horizontal_offset as usize;
                let end_col = (start_col + self.local_width as usize).min(row.len());

                // Build a small preview of row text for rule detection (first 120 chars)
                if render_debug_enabled {
                    let mut preview = String::new();
                    for col in start_col..end_col.min(start_col + 120) {
                        if let Some(cell) = row.get(col) {
                            preview.push(cell.char);
                        }
                    }
                    let trimmed = preview.trim_end();
                    if trimmed.chars().filter(|&ch| ch == '─').count() > 10 {
                        rule_row_abs.get_or_insert(row_idx as u16);
                    }
                }

                for col_idx in start_col..end_col {
                    if let Some(cell) = row.get(col_idx) {
                        let mut style = cell_to_style(cell);

                        // Apply underline if this position has a prediction
                        let row_pos = row_idx - render_start;
                        let col_pos = col_idx - start_col;
                        if predictions.contains(&(col_pos as u16, row_pos as u16)) {
                            style = style.add_modifier(Modifier::UNDERLINED);
                        }

                        spans.push(Span::styled(cell.char.to_string(), style));
                    }
                }

                lines.push(Line::from(spans));
            } else {
                lines.push(Line::from(""));
            }
        }

        // If we found a rule row, log a seam window around it for visual debugging
        if render_debug_enabled {
            if let Some(r_abs) = rule_row_abs {
                let start = r_abs.saturating_sub(2);
                let end = (r_abs + 2).min(self.grid.height.saturating_sub(1));
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("/tmp/render-debug.log")
                {
                    use std::io::Write;
                    let _ = writeln!(file, "Seam window around rule at row {} (abs):", r_abs);
                    for row in start..=end {
                        let mut line_text = String::new();
                        for col in 0..self.grid.width.min(120) {
                            if let Some(cell) = self.grid.get_cell(row, col) {
                                line_text.push(cell.char);
                            }
                        }
                        let _ = writeln!(file, "  Row {}: '{}'", row, line_text.trim_end());
                    }
                }
            }
        }

        Text::from(lines)
    }

    /// Render the grid content as text with predictive underlines for specific area height, returning the render offset
    fn render_content_with_predictions_for_area_with_offset(
        &self,
        predictions: &[(u16, u16)],
        area_height: u16,
    ) -> (Text<'static>, usize) {
        let mut lines = Vec::new();

        let render_debug_enabled = std::env::var("BEACH_RENDER_DEBUG").ok().is_some();
        // Debug output to file (gated)
        if render_debug_enabled {
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/render-debug.log")
            {
                use std::io::Write;
                let _ = writeln!(file, "=== Render Debug ===");
                let _ = writeln!(file, "area_height: {}", area_height);
                let _ = writeln!(file, "grid.cells.len(): {}", self.grid.cells.len());
                let _ = writeln!(file, "local_height: {}", self.local_height);
                let _ = writeln!(file, "scroll_offset: {}", self.scroll_offset);
            }
        }

        // Use the actual area height for visible rows
        let visible_height = area_height as usize;

        // Calculate starting row
        // IMPORTANT: Terminals anchor content at the BOTTOM, not the top
        // When scroll_offset = 0, we should show the bottom-most content
        let grid_height = self.grid.cells.len();

        // Determine start row based on content and scrolling
        // In realtime mode clamp local scroll within window; in historical mode map to grid content
        let start_row = if grid_height > visible_height {
            let bottom_start = grid_height - visible_height;
            if self.is_historical_mode() {
                // FIXED: In historical mode, simply apply scroll_offset to bottom_start
                // The grid already contains all the data we need, no need for complex view_line math
                let effective_offset = (self.scroll_offset as usize).min(bottom_start);
                bottom_start.saturating_sub(effective_offset)
            } else {
                let effective_offset = (self.scroll_offset as usize).min(bottom_start);
                bottom_start.saturating_sub(effective_offset)
            }
        } else {
            0
        };

        let mut render_start = start_row;
        let mut render_end = (render_start + visible_height).min(grid_height);

        // Adjust viewport to reduce trailing blank lines
        if grid_height > visible_height {
            // Count trailing blank lines
            let mut trailing_blanks = 0;
            'scan: for row_idx in (render_start..render_end).rev() {
                if let Some(row) = self.grid.cells.get(row_idx) {
                    let is_blank = row.iter().all(|cell| {
                        let ch = cell.char;
                        ch == ' ' || ch == '\0' || ch == '\u{00A0}'
                    });
                    if is_blank {
                        trailing_blanks += 1;
                    } else {
                        break 'scan;
                    }
                } else {
                    // Treat missing rows as blank
                    trailing_blanks += 1;
                }
            }

            if trailing_blanks > 1 {
                // Shift window up by the extra blank count (leave 1 blank at bottom)
                let extra = trailing_blanks - 1;
                let shift = extra.min(render_start);
                if shift > 0 {
                    // Safe to shift the viewport up
                    render_start -= shift;
                    render_end -= shift;
                }
            }
        }

        // Debug logging: track rendering decisions
        if let Ok(mut debug_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/beach-debug.log")
        {
            use std::io::Write;
            let bottom_start = if grid_height > visible_height {
                grid_height - visible_height
            } else {
                0
            };
            let effective_offset = (self.scroll_offset as usize).min(bottom_start);

            let _ = writeln!(
                debug_file,
                "[{}] [RENDER] scroll_offset={}, is_historical={}, visible_height={}, grid_height={}, bottom_start={}, effective_offset={}, start_row={}, end_row={}",
                chrono::Utc::now().format("%H:%M:%S%.3f"),
                self.scroll_offset,
                self.is_historical_mode(),
                visible_height,
                grid_height,
                bottom_start,
                effective_offset,
                render_start,
                render_end
            );

            // Log the first few characters of the first rendered row to see what content we're actually displaying
            if let Some(row) = self.grid.cells.get(render_start) {
                let preview: String = row.iter().take(40).map(|c| c.char).collect();
                let _ = writeln!(
                    debug_file,
                    "[{}] [RENDER] First row content (row {}): '{}'",
                    chrono::Utc::now().format("%H:%M:%S%.3f"),
                    render_start,
                    preview.trim_end()
                );
            }
        }

        // More debug output (gated)
        if render_debug_enabled {
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/render-debug.log")
            {
                use std::io::Write;
                let _ = writeln!(file, "visible_height: {}", visible_height);
                let _ = writeln!(file, "start_row: {}", render_start);
                let _ = writeln!(file, "end_row: {}", render_end);
                let _ = writeln!(file, "Rendering rows {} to {}", render_start, render_end);
            }
        }

        // Detect a rule line (heavy box drawing) within the render window and log a small seam window
        // Note: This is diagnostic-only to catch potential visual double-blanks
        let mut rule_row_abs: Option<u16> = None;

        for row_idx in render_start..render_end {
            if let Some(row) = self.grid.cells.get(row_idx) {
                let mut spans = Vec::new();

                // Debug: Log first few chars of each row
                if render_debug_enabled {
                    if row_idx < 3 {
                        if let Ok(mut file) = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open("/tmp/render-debug.log")
                        {
                            use std::io::Write;
                            let _ = writeln!(
                                file,
                                "Row {}: '{}...{}'",
                                row_idx,
                                row.iter().take(5).map(|c| c.char).collect::<String>(),
                                row.iter().rev().take(5).map(|c| c.char).collect::<String>()
                            );
                        }
                    }
                }

                // Detect box drawing rule line (for debug logging)
                let has_rule = row.iter().any(|cell| {
                    let ch = cell.char;
                    ch == '━' || ch == '═' || ch == '─'
                });

                if has_rule && rule_row_abs.is_none() {
                    rule_row_abs = Some(row_idx as u16);
                }

                for (col_idx, cell) in row.iter().enumerate() {
                    let ch = if cell.char == '\0' { ' ' } else { cell.char };
                    let mut style = cell_to_style(cell);

                    // Check if this is the cursor position and we have predictions
                    let should_underline = !predictions.is_empty()
                        && row_idx as u16 == self.grid.cursor.row
                        && col_idx as u16 == self.grid.cursor.col;

                    if should_underline {
                        style = style.add_modifier(Modifier::UNDERLINED);
                    }

                    spans.push(Span::styled(ch.to_string(), style));
                }

                lines.push(Line::from(spans));
            } else {
                // Empty line for missing rows
                lines.push(Line::from(""));
            }
        }

        // Log seam window around rule lines for diagnostic purposes (gated)
        if render_debug_enabled {
            if let Some(r_abs) = rule_row_abs {
                let start = r_abs.saturating_sub(2);
                let end = (r_abs + 2).min(self.grid.height - 1);
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("/tmp/render-debug.log")
                {
                    use std::io::Write;
                    let _ = writeln!(file, "Seam window around rule at row {} (abs):", r_abs);
                    for row in start..=end {
                        let mut line_text = String::new();
                        for col in 0..self.grid.width.min(120) {
                            if let Some(cell) = self.grid.get_cell(row, col) {
                                line_text.push(cell.char);
                            }
                        }
                        let _ = writeln!(file, "  Row {}: '{}'", row, line_text.trim_end());
                    }
                }
            }
        }

        (Text::from(lines), render_start)
    }

    /// Render the grid content as text with predictive underlines (legacy)
    fn render_content_with_predictions(&self, predictions: &[(u16, u16)]) -> Text<'static> {
        let mut lines = Vec::new();

        // Calculate visible range with scroll offset
        // Note: We render up to local_height - 1 to account for status line
        // scroll_offset = 0 means showing the bottom (most recent)
        // scroll_offset > 0 means we've scrolled up to show earlier content
        let visible_height = (self.local_height - 1) as usize; // Reserve 1 line for status

        // Calculate starting row based on scroll offset from bottom
        let grid_height = self.grid.cells.len();
        let start_row = if grid_height > visible_height {
            // In realtime mode, show bottom of current grid; in historical mode, map to correct offset
            let bottom_start = grid_height - visible_height;
            if self.is_historical_mode() {
                // Find the right offset within the grid based on view_line
                if let (Some(grid_start), Some(view_line)) =
                    (self.grid.start_line.to_u64(), self.view_line)
                {
                    let line_offset_in_grid = view_line.saturating_sub(grid_start);
                    (line_offset_in_grid as usize).min(bottom_start)
                } else {
                    0 // Fallback to top if we can't determine offset
                }
            } else {
                bottom_start
            }
        } else {
            // If grid fits entirely, always start at 0
            0
        };

        let end_row = (start_row + visible_height).min(grid_height);

        for row_idx in start_row..end_row {
            if let Some(row) = self.grid.cells.get(row_idx) {
                let mut spans = Vec::new();

                // Handle horizontal scrolling
                let start_col = self.horizontal_offset as usize;
                let end_col = (start_col + self.local_width as usize).min(row.len());

                for col_idx in start_col..end_col {
                    if let Some(cell) = row.get(col_idx) {
                        let mut style = cell_to_style(cell);

                        // Apply underline if this position has a prediction
                        let row_pos = row_idx - start_row;
                        let col_pos = col_idx - start_col;
                        if predictions.contains(&(col_pos as u16, row_pos as u16)) {
                            style = style.add_modifier(Modifier::UNDERLINED);
                        }

                        spans.push(Span::styled(cell.char.to_string(), style));
                    }
                }

                lines.push(Line::from(spans));
            } else {
                lines.push(Line::from(""));
            }
        }

        Text::from(lines)
    }

    /// Render the status line with scroll indicators
    fn render_status_line(&self) -> Paragraph<'static> {
        let mut status = String::new();

        // Show horizontal scroll indicator if needed
        if self.needs_horizontal_scroll() {
            status.push_str(&format!(
                "← Col {}/{} → ",
                self.horizontal_offset + 1,
                self.server_width
            ));
        }

        // Show server dimensions if different from local
        if self.server_width != self.local_width || self.server_height != self.local_height {
            status.push_str(&format!(
                "Server: {}×{} | Local: {}×{}",
                self.server_width, self.server_height, self.local_width, self.local_height
            ));
        }

        // Show scroll position more subtly
        if self.scroll_offset > 0 {
            if let Some(metadata) = &self.history_metadata {
                // Show actual line numbers when metadata is available
                let current_top_line = if let Some(grid_end) = self.grid.end_line.to_u64() {
                    grid_end
                        .saturating_sub((self.local_height - 1) as u64)
                        .saturating_sub(self.scroll_offset as u64)
                } else {
                    metadata
                        .latest_line
                        .saturating_sub(self.scroll_offset as u64)
                };
                status.push_str(&format!(
                    " | Scroll: Line {} (offset {})",
                    current_top_line, self.scroll_offset
                ));
            } else {
                // Fallback to offset-based display
                let grid_height = self.grid.cells.len();
                let visible_height = (self.local_height - 1) as usize;
                let max_scroll = grid_height.saturating_sub(visible_height) as u16;

                status.push_str(&format!(
                    " | Scroll: {}/{}",
                    self.scroll_offset + 1,
                    max_scroll + 1
                ));
            }
        }

        // Indicate when scrollback is pending in realtime mode
        if !self.is_historical_mode() && self.scroll_offset > 0 {
            status.push_str(" | Loading history…");
        }

        Paragraph::new(status).style(Style::default().fg(RatatuiColor::Gray))
    }

    /// Render scrollback indicator overlay
    fn render_scrollback_overlay(&self, frame: &mut Frame, area: ratatui::layout::Rect) {
        // Create overlay text with scroll information
        let mut overlay_text = String::new();

        // Show current position info with better line calculation
        if let Some(metadata) = &self.history_metadata {
            // Calculate actual line being viewed
            let current_top_line = if let Some(grid_end) = self.grid.end_line.to_u64() {
                // Use grid end line and adjust for scroll
                grid_end
                    .saturating_sub((self.local_height - 1) as u64)
                    .saturating_sub(self.scroll_offset as u64)
            } else {
                // Fallback to metadata
                metadata
                    .latest_line
                    .saturating_sub(self.scroll_offset as u64)
            };

            // Show range of visible lines
            let lines_shown = (self.local_height - 1) as u64; // -1 for status line if any
            let current_bottom_line =
                (current_top_line + lines_shown - 1).min(metadata.latest_line);

            if current_top_line == current_bottom_line {
                overlay_text = format!("SCROLLBACK: Line {} | ESC to exit", current_top_line);
            } else {
                overlay_text = format!(
                    "SCROLLBACK: Lines {}-{} | ESC to exit",
                    current_top_line, current_bottom_line
                );
            }
        } else {
            overlay_text = format!("SCROLLBACK: Offset {} | ESC to exit", self.scroll_offset);
        }

        // Calculate position for top-right corner
        let overlay_width = overlay_text.len() as u16 + 2; // +2 for padding
        if area.width > overlay_width {
            let overlay_area = ratatui::layout::Rect {
                x: area.x + area.width - overlay_width,
                y: area.y,
                width: overlay_width,
                height: 1,
            };

            // Create paragraph with dark background and light text for visibility
            let overlay_paragraph = Paragraph::new(format!(" {} ", overlay_text)).style(
                Style::default()
                    .fg(RatatuiColor::White)
                    .bg(RatatuiColor::DarkGray),
            );

            frame.render_widget(overlay_paragraph, overlay_area);
        }
    }

    /// Keep the last snapshot for resilience
    pub fn retain_last_snapshot(&mut self) {
        if let Some(snapshot) = &self.last_snapshot {
            self.grid = snapshot.clone();
        }
    }

    /// Apply a predictive character input locally for immediate feedback
    pub fn apply_predictive_input(&mut self, c: char, cursor_pos: (u16, u16)) {
        let (col, row) = cursor_pos;

        // Only apply if cursor is within bounds
        if row < self.grid.height && col < self.grid.width {
            // Create a cell with the character (using default style)
            let cell = crate::server::terminal_state::Cell {
                char: c,
                fg_color: crate::server::terminal_state::Color::Default,
                bg_color: crate::server::terminal_state::Color::Default,
                attributes: crate::server::terminal_state::CellAttributes::default(),
            };

            // Set the cell in the grid
            self.grid.set_cell(row, col, cell);

            // Advance cursor position if we're not at the end of line
            if col + 1 < self.grid.width {
                self.grid.cursor.col = col + 1;
            }
        }
    }
}

/// Convert a Cell to ratatui Style
fn cell_to_style(cell: &Cell) -> Style {
    let mut style = Style::default();

    // Apply foreground color
    style = style.fg(color_to_ratatui(&cell.fg_color));

    // Apply background color
    style = style.bg(color_to_ratatui(&cell.bg_color));

    // Apply text modifiers
    if cell.attributes.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.attributes.italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.attributes.underline {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.attributes.reverse {
        style = style.add_modifier(Modifier::REVERSED);
    }
    if cell.attributes.dim {
        style = style.add_modifier(Modifier::DIM);
    }

    style
}

/// Convert our Color enum to ratatui Color
fn color_to_ratatui(color: &Color) -> RatatuiColor {
    match color {
        Color::Default => RatatuiColor::Reset,
        Color::Indexed(idx) => indexed_to_ratatui(*idx),
        Color::Rgb(r, g, b) => RatatuiColor::Rgb(*r, *g, *b),
    }
}

/// Convert indexed color to ratatui Color
fn indexed_to_ratatui(idx: u8) -> RatatuiColor {
    match idx {
        0 => RatatuiColor::Black,
        1 => RatatuiColor::Red,
        2 => RatatuiColor::Green,
        3 => RatatuiColor::Yellow,
        4 => RatatuiColor::Blue,
        5 => RatatuiColor::Magenta,
        6 => RatatuiColor::Cyan,
        7 => RatatuiColor::White,
        8 => RatatuiColor::DarkGray,
        9 => RatatuiColor::LightRed,
        10 => RatatuiColor::LightGreen,
        11 => RatatuiColor::LightYellow,
        12 => RatatuiColor::LightBlue,
        13 => RatatuiColor::LightMagenta,
        14 => RatatuiColor::LightCyan,
        15 => RatatuiColor::Gray,
        // For 256-color palette, approximate with RGB
        16..=255 => {
            // This is a simplified conversion; full 256-color palette would need proper mapping
            RatatuiColor::Indexed(idx)
        }
    }
}

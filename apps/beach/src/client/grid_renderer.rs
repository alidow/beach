/// Grid renderer for terminal client
/// Maintains local terminal grid matching server dimensions
use crate::server::terminal_state::{Grid, GridDelta, Cell, Color};
use crate::subscription::HistoryMetadata;
use crossterm::terminal;
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color as RatatuiColor, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use std::collections::{BTreeMap, HashSet};
use std::io;

/// Historical line data for cache
#[derive(Clone, Debug)]
pub struct HistoryLine {
    /// The actual cell content
    pub cells: Vec<Cell>,
    /// Absolute line number in terminal history
    pub line_number: u64,
}

/// Request for historical lines
#[derive(Clone, Debug)]
pub struct HistoryRequest {
    /// Start line number (inclusive)
    pub start_line: u64,
    /// End line number (inclusive)
    pub end_line: u64,
}

/// Manages the local terminal grid and rendering
pub struct GridRenderer {
    /// Server grid dimensions (authoritative)
    pub server_width: u16,
    pub server_height: u16,
    
    /// Local terminal dimensions
    local_width: u16,
    local_height: u16,
    
    /// Current grid state (realtime view)
    pub grid: Grid,
    
    /// History cache for scrollback (line_number -> HistoryLine)
    history_cache: BTreeMap<u64, HistoryLine>,
    
    /// Pending history requests to avoid duplicates
    pending_requests: HashSet<String>,
    
    /// History metadata from server
    history_metadata: Option<HistoryMetadata>,
    
    /// Current view line number (for historical mode)
    /// None = realtime mode, Some(n) = viewing from line n
    view_line: Option<u64>,
    
    /// Vertical scroll position (0 = bottom of output)
    pub scroll_offset: u16,
    
    /// Horizontal scroll position (for when local < server width)
    horizontal_offset: u16,
    
    /// Buffer for overscan (2x visible height)
    overscan_buffer: Vec<Grid>,
    
    /// Line index in server history
    from_line: u64,
    
    /// Last rendered snapshot for resilience
    last_snapshot: Option<Grid>,
    
    /// Whether to show the debug size status line
    debug_size: bool,
}

impl GridRenderer {
    /// Create a new grid renderer
    pub fn new(server_width: u16, server_height: u16, debug_size: bool) -> io::Result<Self> {
        let (local_width, local_height) = terminal::size()?;
        
        let grid = Grid::new(server_width, server_height);
        
        Ok(Self {
            server_width,
            server_height,
            local_width,
            local_height,
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
        })
    }
    
    /// Update local terminal dimensions
    pub fn resize_local(&mut self, width: u16, height: u16) {
        self.local_width = width;
        self.local_height = height;
    }
    
    /// Apply a snapshot from the server
    pub fn apply_snapshot(&mut self, snapshot: Grid) {
        // Debug logging at start of apply_snapshot
        if let Ok(mut debug_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/beach-snapshot-debug.log")
        {
            use std::io::Write;
            let _ = writeln!(debug_file, 
                "[{}] [APPLY_SNAPSHOT] Starting:",
                chrono::Utc::now()
            );
            let _ = writeln!(debug_file, 
                "  self.view_line BEFORE: {:?}",
                self.view_line
            );
            let _ = writeln!(debug_file, 
                "  snapshot.start_line: {:?}",
                snapshot.start_line.to_u64()
            );
            let _ = writeln!(debug_file, 
                "  is_historical check: {}",
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
            // Update our historical anchor to the snapshot's start and reset local offset
            if let Some(s) = snapshot.start_line.to_u64() {
                self.view_line = Some(s);
                self.scroll_offset = 0;
            }
        }
        
        self.grid = snapshot.clone();
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
            .open("/tmp/beach-snapshot-debug.log")
        {
            use std::io::Write;
            let _ = writeln!(debug_file, 
                "[{}] [APPLY_SNAPSHOT] self.view_line AFTER: {:?}",
                chrono::Utc::now(),
                self.view_line
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
            self.grid.cells.resize(new_height, vec![Cell::default(); new_width]);
            
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
        // Delta convention:
        // Positive delta = scroll content up (view earlier/older content)
        // Negative delta = scroll content down (view later/newer content)
        
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
                self.scroll_offset.saturating_add(delta as u16).min(max_scroll)
            } else {
                self.scroll_offset.saturating_sub(delta.abs() as u16)
            };
            
            self.scroll_offset = new_offset;
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
            let _ = writeln!(file, 
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
                    self.grid.get_cell(row, col)
                        .map(|c| c.char == ' ' || c.char == '\0' || c.char == '\u{00A0}')
                        .unwrap_or(true)
                });
                if is_blank { trailing_blanks += 1; } else { break; }
            }
            let effective_end_line_adj = effective_end_line.saturating_sub(trailing_blanks);
            let current_top_line = effective_end_line_adj
                .saturating_sub(self.local_height as u64 - 1); // Approximate top line
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
            let request = HistoryRequest { start_line, end_line };
            
            // Debug log the request
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/calculate-history-debug.log")
            {
                use std::io::Write;
                let _ = writeln!(file, 
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
            return Some(HistoryRequest { start_line: target_line, end_line });
        }
        
        None
    }
    
    /// Switch to historical view mode at a specific line
    pub fn enter_historical_mode(&mut self, line_number: u64) {
        // Debug logging for mode transition
        if let Ok(mut debug_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/beach-state-flow.log")
        {
            use std::io::Write;
            let _ = writeln!(debug_file, 
                "[{}] [ENTER_HISTORICAL_MODE] view_line: {:?} -> Some({})",
                chrono::Utc::now(),
                self.view_line,
                line_number
            );
        }
        
        self.view_line = Some(line_number);
        // Reset local scroll so historical view starts anchored at requested line
        self.scroll_offset = 0;
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
            let _ = writeln!(debug_file, 
                "[{}] [ENTER_REALTIME_MODE] view_line: {:?} -> None",
                chrono::Utc::now(),
                self.view_line
            );
        }
        
        self.view_line = None;
        self.scroll_offset = 0;
    }
    
    /// Check if we're in historical view mode
    pub fn is_historical_mode(&self) -> bool {
        self.view_line.is_some()
    }
    
    /// Render the grid to a ratatui frame with optional predictive underlines
    pub fn render(&self, frame: &mut Frame, predictions: &[(u16, u16)]) {
        let area = frame.area();
        
        // Enhanced render state logging
        if let Ok(mut debug_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/beach-render-state.log")
        {
            use std::io::Write;
            let _ = writeln!(debug_file, 
                "[{}] [RENDER] State check:",
                chrono::Utc::now()
            );
            let _ = writeln!(debug_file, 
                "  view_line: {:?}",
                self.view_line
            );
            let _ = writeln!(debug_file, 
                "  is_historical_mode(): {}",
                self.is_historical_mode()
            );
            let _ = writeln!(debug_file, 
                "  scroll_offset: {}",
                self.scroll_offset
            );
            let _ = writeln!(debug_file, 
                "  history_metadata: {:?}",
                self.history_metadata
            );
            let _ = writeln!(debug_file, 
                "  grid_dims: {}x{}, area_dims: {}x{}",
                self.grid.width, self.grid.height,
                area.width, area.height
            );
            let _ = writeln!(debug_file, 
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
        let (content, render_start) = self.render_content_with_predictions_for_area_with_offset(predictions, chunks[0].height);
        // Disable wrapping for sanity check: each grid row maps to exactly one drawn row
        let paragraph = Paragraph::new(content)
            .block(Block::default().borders(Borders::NONE));
        frame.render_widget(paragraph, chunks[0]);
        
        // Render cursor if visible
        if self.grid.cursor.visible && self.scroll_offset == 0 {
            // Calculate the cursor position accounting for scrolling
            let cursor_row = self.grid.cursor.row;
            let cursor_col = self.grid.cursor.col;
            
            // Calculate visible range
            let visible_height = chunks[0].height as usize;
            
            // Check if cursor is within visible area using the actual render_start from content rendering
            if cursor_row as usize >= render_start && (cursor_row as usize) < render_start + visible_height {
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
        
        // Render status line only if debug flag is set
        if show_status_line && chunks.len() > 1 {
            let status = self.render_status_line();
            frame.render_widget(status, chunks[1]);
        }
    }
    
    /// Render the grid content as text with predictive underlines for specific area height
    fn render_content_with_predictions_for_area(&self, predictions: &[(u16, u16)], area_height: u16) -> Text<'static> {
        let mut lines = Vec::new();
        
        let render_debug_enabled = std::env::var("BEACH_RENDER_DEBUG").ok().is_some();
        // Debug output to file (gated)
        if render_debug_enabled {
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/render-debug.log") {
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
        // Bottom-anchor by default; in realtime mode clamp the local scroll
        // within the available window so the user sees immediate feedback.
        let start_row = if grid_height > visible_height {
            let bottom_start = grid_height - visible_height;
            if self.is_historical_mode() {
                0
            } else {
                let effective_offset = (self.scroll_offset as usize).min(bottom_start);
                bottom_start.saturating_sub(effective_offset)
            }
        } else {
            0
        };
        
        let mut render_start = start_row;
        let mut render_end = (render_start + visible_height).min(grid_height);
        
        // Debug log the rendering range
        if let Ok(mut debug_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/beach-render-debug.log")
        {
            use std::io::Write;
            let _ = writeln!(debug_file, 
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
                let max_shift = grid_height.saturating_sub(visible_height).saturating_sub(render_start);
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
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/render-debug.log") {
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
                        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/render-debug.log") {
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
                if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/render-debug.log") {
                    use std::io::Write;
                    let _ = writeln!(file, "Seam window around rule at row {} (abs):", r_abs);
                    for row in start..=end {
                        let mut line_text = String::new();
                        for col in 0..self.grid.width.min(120) {
                            if let Some(cell) = self.grid.get_cell(row, col) { line_text.push(cell.char); }
                        }
                        let _ = writeln!(file, "  Row {}: '{}'", row, line_text.trim_end());
                    }
                }
            }
        }
        
        Text::from(lines)
    }
    
    /// Render the grid content as text with predictive underlines for specific area height, returning the render offset
    fn render_content_with_predictions_for_area_with_offset(&self, predictions: &[(u16, u16)], area_height: u16) -> (Text<'static>, usize) {
        let mut lines = Vec::new();
        
        let render_debug_enabled = std::env::var("BEACH_RENDER_DEBUG").ok().is_some();
        // Debug output to file (gated)
        if render_debug_enabled {
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/render-debug.log") {
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
        // Bottom-anchor by default; in realtime clamp local scroll within window
        let start_row = if grid_height > visible_height {
            let bottom_start = grid_height - visible_height;
            if self.is_historical_mode() {
                0
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
        
        // More debug output (gated)
        if render_debug_enabled {
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/render-debug.log") {
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
                        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/render-debug.log") {
                            use std::io::Write;
                            let _ = writeln!(file, "Row {}: '{}...{}'", 
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
                    
                    // Check if this position is in the predictions list and apply underline
                    let should_underline = predictions.iter().any(|&(pred_row, pred_col)| {
                        pred_row as usize == row_idx && pred_col as usize == col_idx
                    });
                    
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
                if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/render-debug.log") {
                    use std::io::Write;
                    let _ = writeln!(file, "Seam window around rule at row {} (abs):", r_abs);
                    for row in start..=end {
                        let mut line_text = String::new();
                        for col in 0..self.grid.width.min(120) {
                            if let Some(cell) = self.grid.get_cell(row, col) { line_text.push(cell.char); }
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
            // In realtime mode, show bottom of current grid; in historical mode, show from top
            // of the returned snapshot. Local vertical scrolling is driven via history requests.
            let bottom_start = grid_height - visible_height;
            if self.is_historical_mode() { 0 } else { bottom_start }
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
                self.server_width,
                self.server_height,
                self.local_width,
                self.local_height
            ));
        }
        
        // Show scroll position more subtly
        if self.scroll_offset > 0 {
            let grid_height = self.grid.cells.len();
            let visible_height = (self.local_height - 1) as usize;
            let max_scroll = grid_height.saturating_sub(visible_height) as u16;
            
            status.push_str(&format!(
                " | Line {}/{}",
                self.scroll_offset + 1,
                max_scroll + 1
            ));
        }
        
        // Indicate when scrollback is pending in realtime mode
        if !self.is_historical_mode() && self.scroll_offset > 0 {
            status.push_str(" | Loading history…");
        }
        
        Paragraph::new(status)
            .style(Style::default().fg(RatatuiColor::Gray))
    }
    
    /// Keep the last snapshot for resilience
    pub fn retain_last_snapshot(&mut self) {
        if let Some(snapshot) = &self.last_snapshot {
            self.grid = snapshot.clone();
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

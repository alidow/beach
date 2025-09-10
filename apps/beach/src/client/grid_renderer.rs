/// Grid renderer for terminal client
/// Maintains local terminal grid matching server dimensions
use crate::server::terminal_state::{Grid, GridDelta, Cell, Color};
use crossterm::terminal;
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color as RatatuiColor, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use std::io;

/// Manages the local terminal grid and rendering
pub struct GridRenderer {
    /// Server grid dimensions (authoritative)
    pub server_width: u16,
    pub server_height: u16,
    
    /// Local terminal dimensions
    local_width: u16,
    local_height: u16,
    
    /// Current grid state
    pub grid: Grid,
    
    /// Vertical scroll position (0 = bottom of output)
    scroll_offset: u16,
    
    /// Horizontal scroll position (for when local < server width)
    horizontal_offset: u16,
    
    /// Buffer for overscan (2x visible height)
    overscan_buffer: Vec<Grid>,
    
    /// Line index in server history
    from_line: u64,
    
    /// Last rendered snapshot for resilience
    last_snapshot: Option<Grid>,
}

impl GridRenderer {
    /// Create a new grid renderer
    pub fn new(server_width: u16, server_height: u16) -> io::Result<Self> {
        let (local_width, local_height) = terminal::size()?;
        
        let grid = Grid::new(server_width, server_height);
        
        Ok(Self {
            server_width,
            server_height,
            local_width,
            local_height,
            grid,
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
    
    /// Apply a snapshot from the server
    pub fn apply_snapshot(&mut self, snapshot: Grid) {
        // Update server dimensions from snapshot
        self.server_width = snapshot.width;
        self.server_height = snapshot.height;
        
        self.grid = snapshot.clone();
        self.last_snapshot = Some(snapshot);
        self.scroll_offset = 0; // Reset to bottom
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
        // scroll_offset = 0 means showing from row 0 (top/oldest)
        // Higher offset means we've scrolled down to show later rows
        
        // Calculate visible height (accounting for status line)
        let visible_height = (self.local_height - 1) as usize;
        
        // Calculate max scroll - can't scroll past the last row
        let grid_height = self.grid.cells.len();
        let max_scroll = grid_height.saturating_sub(visible_height) as u16;
        
        // Apply delta
        let new_offset = if delta > 0 {
            // Positive delta - scroll up to show earlier content (decrease offset)
            self.scroll_offset.saturating_sub(delta as u16)
        } else {
            // Negative delta - scroll down to show later content (increase offset)  
            (self.scroll_offset + delta.abs() as u16).min(max_scroll)
        };
        
        self.scroll_offset = new_offset;
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
    
    /// Render the grid to a ratatui frame with optional predictive underlines
    pub fn render(&self, frame: &mut Frame, predictions: &[(u16, u16)]) {
        let area = frame.area();
        
        // Create main layout
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(1), // Status line
            ])
            .split(area);
        
        // Render the terminal content with predictions
        // Pass the actual content area height so we render the right number of lines
        let content = self.render_content_with_predictions_for_area(predictions, chunks[0].height);
        let paragraph = Paragraph::new(content)
            .block(Block::default().borders(Borders::NONE))
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, chunks[0]);
        
        // Render status line
        let status = self.render_status_line();
        frame.render_widget(status, chunks[1]);
    }
    
    /// Render the grid content as text with predictive underlines for specific area height
    fn render_content_with_predictions_for_area(&self, predictions: &[(u16, u16)], area_height: u16) -> Text<'static> {
        let mut lines = Vec::new();
        
        // Debug output to file
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/render-debug.log") {
            use std::io::Write;
            let _ = writeln!(file, "=== Render Debug ===");
            let _ = writeln!(file, "area_height: {}", area_height);
            let _ = writeln!(file, "grid.cells.len(): {}", self.grid.cells.len());
            let _ = writeln!(file, "local_height: {}", self.local_height);
            let _ = writeln!(file, "scroll_offset: {}", self.scroll_offset);
        }
        
        // Use the actual area height for visible rows
        let visible_height = area_height as usize;
        
        // Calculate starting row
        // IMPORTANT: In terminal grids, content often starts at the bottom with empty rows above
        // We should always show from row 0 unless we're scrolling
        let grid_height = self.grid.cells.len();
        
        // Always start from the beginning unless scrolled
        let start_row = self.scroll_offset as usize;
        
        let end_row = (start_row + visible_height).min(grid_height);
        
        // More debug output
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/render-debug.log") {
            use std::io::Write;
            let _ = writeln!(file, "visible_height: {}", visible_height);
            let _ = writeln!(file, "start_row: {}", start_row);
            let _ = writeln!(file, "end_row: {}", end_row);
            let _ = writeln!(file, "Rendering rows {} to {}", start_row, end_row);
        }
        
        for row_idx in start_row..end_row {
            if let Some(row) = self.grid.cells.get(row_idx) {
                let mut spans = Vec::new();
                
                // Debug: Log first few chars of each row
                if row_idx < 3 {
                    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/render-debug.log") {
                        use std::io::Write;
                        let row_text: String = row.iter().take(40).map(|c| c.char).collect();
                        let _ = writeln!(file, "Row {}: '{}'", row_idx, row_text.trim_end());
                    }
                }
                
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
            // If grid is larger than visible area, apply scroll offset
            let bottom_start = grid_height - visible_height;
            bottom_start.saturating_sub(self.scroll_offset as usize)
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
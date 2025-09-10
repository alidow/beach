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
            // Note: Grid resize would need to be implemented
        }
    }
    
    /// Scroll vertically
    pub fn scroll_vertical(&mut self, delta: i16) {
        let new_offset = (self.scroll_offset as i16 + delta).max(0) as u16;
        self.scroll_offset = new_offset.min(self.overscan_buffer.len() as u16);
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
        let content = self.render_content_with_predictions(predictions);
        let paragraph = Paragraph::new(content)
            .block(Block::default().borders(Borders::NONE))
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, chunks[0]);
        
        // Render status line
        let status = self.render_status_line();
        frame.render_widget(status, chunks[1]);
    }
    
    /// Render the grid content as text with predictive underlines
    fn render_content_with_predictions(&self, predictions: &[(u16, u16)]) -> Text<'static> {
        let mut lines = Vec::new();
        
        // Calculate visible range with scroll offset
        let start_row = self.scroll_offset as usize;
        let end_row = (start_row + self.local_height as usize).min(self.grid.cells.len());
        
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
        
        // Show vertical scroll position
        if !self.overscan_buffer.is_empty() {
            status.push_str(&format!(
                "Line {}/{}",
                self.from_line + self.scroll_offset as u64,
                self.from_line + self.overscan_buffer.len() as u64
            ));
        }
        
        // Show server dimensions if different from local
        if self.server_width != self.local_width || self.server_height != self.local_height {
            status.push_str(&format!(
                " | Server: {}×{}",
                self.server_width,
                self.server_height
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
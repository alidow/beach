#[cfg(feature = "alacritty-backend")]
use alacritty_terminal::{
    Term,
    event::{Event, EventListener},
    grid::Dimensions,
    term::Config,
    vte::ansi::Processor,
};
use std::sync::{Arc, Mutex};
use std::io::Write;
use chrono::{DateTime, Utc};
use crate::server::terminal_state::{
    Grid, GridHistory, GridDelta, Cell, Color, CellAttributes, 
    CursorShape, CursorPosition, TerminalInitializer, DimensionChange
};

/// Simple dimensions implementation for alacritty
#[cfg(feature = "alacritty-backend")]
struct TermDimensions {
    columns: usize,
    screen_lines: usize,
}

#[cfg(feature = "alacritty-backend")]
impl Dimensions for TermDimensions {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }
    
    fn screen_lines(&self) -> usize {
        self.screen_lines
    }
    
    fn columns(&self) -> usize {
        self.columns
    }
}

/// Event listener proxy for alacritty terminal
#[cfg(feature = "alacritty-backend")]
#[derive(Clone)]
struct EventProxy;

#[cfg(feature = "alacritty-backend")]
impl EventListener for EventProxy {
    fn send_event(&self, _event: Event) {
        // Handle events if needed
    }
}

/// Wrapper around alacritty_terminal's Term to provide our Grid interface
#[cfg(feature = "alacritty-backend")]
pub struct AlacrittyTerminal {
    term: Arc<Mutex<Term<EventProxy>>>,
    parser: Arc<Mutex<Processor>>,
    current_grid: Grid,
    history: Arc<Mutex<GridHistory>>,
    last_update: DateTime<Utc>,
    event_proxy: EventProxy,
    width: u16,
    height: u16,
    debug_log: Option<Arc<Mutex<std::fs::File>>>,
}

#[cfg(feature = "alacritty-backend")]
impl AlacrittyTerminal {
    pub fn new(width: u16, height: u16, debug_log: Option<&std::fs::File>) -> anyhow::Result<Self> {
        let debug_log = debug_log.map(|f| {
            // Clone the file handle for thread-safe access
            Arc::new(Mutex::new(f.try_clone().expect("Failed to clone debug log file")))
        });
        
        // Log initial dimensions
        if let Some(ref log) = debug_log {
            if let Ok(mut f) = log.lock() {
                use std::io::Write;
                let _ = writeln!(f, "[{}] AlacrittyTerminal::new requested dimensions: {}x{}", 
                                 Utc::now().format("%H:%M:%S%.3f"), width, height);
            }
        }
        
        let dimensions = TermDimensions {
            columns: width as usize,
            screen_lines: height as usize,
        };
        
        let event_proxy = EventProxy;
        let config = Config::default();
        
        // Create terminal
        let term = Term::new(config, &dimensions, event_proxy.clone());
        
        // Log actual dimensions after creation
        if let Some(ref log) = debug_log {
            if let Ok(mut f) = log.lock() {
                use std::io::Write;
                let actual_cols = term.grid().columns();
                let actual_lines = term.grid().screen_lines();
                let _ = writeln!(f, "[{}] AlacrittyTerminal::new actual dimensions after creation: {}x{}", 
                                 Utc::now().format("%H:%M:%S%.3f"), actual_cols, actual_lines);
            }
        }
        
        let term = Arc::new(Mutex::new(term));
        
        // Create ANSI parser
        let parser = Processor::new();
        let parser = Arc::new(Mutex::new(parser));
        
        // Initialize our grid representation using TerminalInitializer
        let initial_grid = TerminalInitializer::create_initial_grid(width, height);
        let history = Arc::new(Mutex::new(GridHistory::new(initial_grid.clone())));
        
        // Create the terminal instance
        let mut terminal = Self {
            term,
            parser,
            current_grid: initial_grid,
            history,
            last_update: Utc::now(),
            event_proxy,
            width,
            height,
            debug_log,
        };
        
        // Enable LINE_FEED_NEW_LINE mode (ESC[20h) 
        // This makes \n perform both line feed and carriage return
        // which is standard Unix terminal behavior
        terminal.process_output(b"\x1b[20h").ok();
        
        Ok(terminal)
    }
    
    pub fn process_output(&mut self, data: &[u8]) -> anyhow::Result<()> {
        // Convert lone \n to \r\n for proper Unix terminal behavior
        // This ensures \n performs both line feed and carriage return
        let mut processed_data = Vec::with_capacity(data.len() * 2);
        let mut i = 0;
        while i < data.len() {
            if data[i] == b'\n' {
                // Check if it's not already preceded by \r
                if i == 0 || data[i - 1] != b'\r' {
                    processed_data.push(b'\r');
                }
            }
            processed_data.push(data[i]);
            i += 1;
        }
        
        // Process the ANSI data through alacritty's parser
        {
            let mut parser = self.parser.lock().unwrap();
            let mut term = self.term.lock().unwrap();
            
            for byte in &processed_data {
                parser.advance(&mut *term, *byte);
            }
        }
        
        // Update our grid from alacritty's state
        self.sync_grid()?;
        
        Ok(())
    }
    
    fn sync_grid(&mut self) -> anyhow::Result<()> {
        let term = self.term.lock().unwrap();
        let old_grid = self.current_grid.clone();
        let old_width = old_grid.width;
        let old_height = old_grid.height;
        
        // Get alacritty's actual grid dimensions
        let grid = term.grid();
        let alac_cols = grid.columns();
        let alac_lines = grid.screen_lines();
        
        // Log dimension mismatch if it occurs
        if let Some(ref log) = self.debug_log {
            if let Ok(mut f) = log.lock() {
                use std::io::Write;
                if alac_cols != self.width as usize || alac_lines != self.height as usize {
                    let _ = writeln!(f, "[{}] sync_grid: Dimension mismatch! Alacritty: {}x{}, Expected: {}x{}", 
                                     Utc::now().format("%H:%M:%S%.3f"), 
                                     alac_cols, alac_lines, self.width, self.height);
                }
            }
        }
        
        // Use our stored dimensions for the output grid
        self.current_grid.width = self.width;
        self.current_grid.height = self.height;
        
        // Ensure the grid cells match the dimensions
        if let Err(e) = self.current_grid.resize(self.width, self.height) {
            if let Some(ref log) = self.debug_log {
                if let Ok(mut f) = log.lock() {
                    use std::io::Write;
                    let _ = writeln!(f, "[{}] process_output: Failed to resize grid: {}", 
                                     Utc::now().format("%H:%M:%S%.3f"), e);
                }
            }
        }
        
        // Sync cursor position
        let cursor_point = term.grid().cursor.point;
        self.current_grid.cursor = CursorPosition {
            row: cursor_point.line.0 as u16,
            col: cursor_point.column.0 as u16,
            visible: term.mode().contains(alacritty_terminal::term::TermMode::SHOW_CURSOR),
            shape: self.map_cursor_shape(&term),
        };
        
        // Sync cells from alacritty's grid
        for line in 0..alac_lines.min(self.height as usize) {
            for col in 0..alac_cols.min(self.width as usize) {
                let point = alacritty_terminal::index::Point {
                    line: alacritty_terminal::index::Line(line as i32),
                    column: alacritty_terminal::index::Column(col),
                };
                
                // Access the cell from the grid
                let cell_ref = &term.grid()[point];
                let cell = self.convert_cell(cell_ref);
                self.current_grid.set_cell(line as u16, col as u16, cell);
            }
        }
        
        // Fill any remaining cells with defaults (if our grid is larger than alacritty's)
        if alac_cols < self.width as usize {
            for line in 0..self.height {
                for col in alac_cols as u16..self.width {
                    self.current_grid.set_cell(line, col, Cell::default());
                }
            }
        }
        
        // Update timestamp
        self.current_grid.timestamp = Utc::now();
        
        // Create delta and update history
        let mut delta = GridDelta::diff(&old_grid, &self.current_grid);
        
        // If dimensions changed, ensure it's recorded in the delta
        if old_width != self.current_grid.width || old_height != self.current_grid.height {
            if delta.dimension_change.is_none() {
                delta.dimension_change = Some(DimensionChange {
                    old_width,
                    old_height,
                    new_width: self.current_grid.width,
                    new_height: self.current_grid.height,
                });
            }
        }
        
        let mut history = self.history.lock().unwrap();
        
        // Always add delta (even if empty) to keep history in sync
        history.add_delta(delta);
        
        // Always add a snapshot for now to ensure tests work
        // TODO: optimize this to only snapshot periodically in production
        history.add_snapshot(self.current_grid.clone());
        
        Ok(())
    }
    
    fn convert_cell(&self, alac_cell: &alacritty_terminal::term::cell::Cell) -> Cell {
        use alacritty_terminal::term::cell::Flags;
        
        // Check if this cell has the WRAPLINE flag
        // This indicates the line wrapped at this position
        let has_wrapline = alac_cell.flags.contains(Flags::WRAPLINE);
        
        // For now, we'll track this for debugging
        // In the future, we may need to preserve this in our Cell structure
        if has_wrapline {
            // This cell marks where a line wrapped
            // The next line is a continuation of this one
        }
        
        Cell {
            char: alac_cell.c,
            fg_color: self.convert_color(&alac_cell.fg),
            bg_color: self.convert_color(&alac_cell.bg),
            attributes: self.convert_attributes(&alac_cell.flags),
        }
    }
    
    fn convert_color(&self, alac_color: &alacritty_terminal::vte::ansi::Color) -> Color {
        use alacritty_terminal::vte::ansi::{Color as AlacColor, NamedColor};
        match alac_color {
            AlacColor::Named(n) => {
                // Map named colors to indexed colors
                let index = match n {
                    NamedColor::Black => 0,
                    NamedColor::Red => 1,
                    NamedColor::Green => 2,
                    NamedColor::Yellow => 3,
                    NamedColor::Blue => 4,
                    NamedColor::Magenta => 5,
                    NamedColor::Cyan => 6,
                    NamedColor::White => 7,
                    NamedColor::BrightBlack => 8,
                    NamedColor::BrightRed => 9,
                    NamedColor::BrightGreen => 10,
                    NamedColor::BrightYellow => 11,
                    NamedColor::BrightBlue => 12,
                    NamedColor::BrightMagenta => 13,
                    NamedColor::BrightCyan => 14,
                    NamedColor::BrightWhite => 15,
                    NamedColor::Foreground => return Color::Default,
                    NamedColor::Background => return Color::Default,
                    _ => return Color::Default,
                };
                Color::Indexed(index)
            },
            AlacColor::Spec(rgb) => {
                // RGB values are already in 0-255 range in 0.21
                Color::Rgb(rgb.r, rgb.g, rgb.b)
            },
            AlacColor::Indexed(i) => Color::Indexed(*i),
        }
    }
    
    fn convert_attributes(&self, flags: &alacritty_terminal::term::cell::Flags) -> CellAttributes {
        use alacritty_terminal::term::cell::Flags;
        
        CellAttributes {
            bold: flags.contains(Flags::BOLD),
            italic: flags.contains(Flags::ITALIC),
            underline: flags.contains(Flags::UNDERLINE) 
                || flags.contains(Flags::DOUBLE_UNDERLINE)
                || flags.contains(Flags::UNDERCURL)
                || flags.contains(Flags::DOTTED_UNDERLINE)
                || flags.contains(Flags::DASHED_UNDERLINE),
            reverse: flags.contains(Flags::INVERSE),
            blink: false, // SLOW_BLINK and RAPID_BLINK don't exist in 0.21
            strikethrough: flags.contains(Flags::STRIKEOUT),
            dim: flags.contains(Flags::DIM),
            hidden: flags.contains(Flags::HIDDEN),
        }
    }
    
    fn map_cursor_shape(&self, _term: &Term<EventProxy>) -> CursorShape {
        // Alacritty doesn't expose cursor style directly in 0.21,
        // default to block for now
        CursorShape::Block
    }
    
    pub fn get_history(&self) -> Arc<Mutex<GridHistory>> {
        Arc::clone(&self.history)
    }
    
    pub fn get_current_grid(&self) -> Grid {
        self.current_grid.clone()
    }
    
    pub fn force_snapshot(&mut self) {
        // Force a snapshot for testing purposes
        let _ = self.sync_grid();
    }
    
    pub fn resize(&mut self, width: u16, height: u16) -> anyhow::Result<()> {
        // Only resize if dimensions actually changed
        if self.width == width && self.height == height {
            return Ok(());
        }
        
        // Log resize request
        if let Some(ref log) = self.debug_log {
            if let Ok(mut f) = log.lock() {
                use std::io::Write;
                let _ = writeln!(f, "[{}] resize: Requested resize from {}x{} to {}x{}", 
                                 Utc::now().format("%H:%M:%S%.3f"), 
                                 self.width, self.height, width, height);
            }
        }
        
        // Create new dimensions
        let dimensions = TermDimensions {
            columns: width as usize,
            screen_lines: height as usize,
        };
        
        // Resize the terminal (preserves content)
        {
            let mut term_lock = self.term.lock().unwrap();
            term_lock.resize(dimensions);
            
            // Check actual dimensions after resize
            let actual_cols = term_lock.grid().columns();
            let actual_lines = term_lock.grid().screen_lines();
            
            if let Some(ref log) = self.debug_log {
                if let Ok(mut f) = log.lock() {
                    use std::io::Write;
                    let _ = writeln!(f, "[{}] resize: After resize, alacritty reports: {}x{}", 
                                     Utc::now().format("%H:%M:%S%.3f"), 
                                     actual_cols, actual_lines);
                    if actual_cols != width as usize || actual_lines != height as usize {
                        let _ = writeln!(f, "[{}] resize: WARNING! Alacritty did not accept the requested dimensions!", 
                                         Utc::now().format("%H:%M:%S%.3f"));
                    }
                }
            }
        }
        
        // Update our stored dimensions
        self.width = width;
        self.height = height;
        
        // Sync the grid to get the resized state
        // sync_grid() will automatically create the delta with dimension change
        self.sync_grid()?;
        
        Ok(())
    }
}

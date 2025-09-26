use crate::debug_recorder::DebugRecorder;
use crate::server::terminal_state::{
    Cell, CellAttributes, Color, CursorPosition, CursorShape, DimensionChange, Grid, GridDelta,
    GridHistory, TerminalInitializer,
};
#[cfg(feature = "alacritty-backend")]
use alacritty_terminal::{
    Term,
    event::{Event, EventListener},
    grid::Dimensions,
    term::Config,
    vte::ansi::Processor,
};
use chrono::{DateTime, Utc};
use std::sync::{Arc, Mutex};

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
    debug_recorder: Option<Arc<Mutex<DebugRecorder>>>,
    process_output_sequence: std::sync::atomic::AtomicU64,
}

#[cfg(feature = "alacritty-backend")]
impl AlacrittyTerminal {
    pub fn new(
        width: u16,
        height: u16,
        debug_log: Option<&std::fs::File>,
        debug_recorder: Option<Arc<Mutex<DebugRecorder>>>,
    ) -> anyhow::Result<Self> {
        let debug_log = debug_log.map(|f| {
            // Clone the file handle for thread-safe access
            Arc::new(Mutex::new(
                f.try_clone().expect("Failed to clone debug log file"),
            ))
        });

        // Log initial dimensions
        if let Some(ref log) = debug_log {
            if let Ok(mut f) = log.lock() {
                use std::io::Write;
                let _ = writeln!(
                    f,
                    "[{}] AlacrittyTerminal::new requested dimensions: {}x{}",
                    Utc::now().format("%H:%M:%S%.3f"),
                    width,
                    height
                );
            }
        }

        let dimensions = TermDimensions {
            columns: width as usize,
            screen_lines: height as usize,
        };

        let event_proxy = EventProxy;
        let mut config = Config::default();

        // Enable scrollback with 10000 lines of history
        config.scrolling_history = 10000;

        // Create terminal
        let term = Term::new(config, &dimensions, event_proxy.clone());

        // Log actual dimensions after creation
        if let Some(ref log) = debug_log {
            if let Ok(mut f) = log.lock() {
                use std::io::Write;
                let actual_cols = term.grid().columns();
                let actual_lines = term.grid().screen_lines();
                let _ = writeln!(
                    f,
                    "[{}] AlacrittyTerminal::new actual dimensions after creation: {}x{}",
                    Utc::now().format("%H:%M:%S%.3f"),
                    actual_cols,
                    actual_lines
                );
            }
        }

        let term = Arc::new(Mutex::new(term));

        // Create ANSI parser
        let parser = Processor::new();
        let parser = Arc::new(Mutex::new(parser));

        // Initialize our grid representation using TerminalInitializer
        let initial_grid = TerminalInitializer::create_initial_grid(width, height);
        let history = Arc::new(Mutex::new(GridHistory::new_with_debug(
            initial_grid.clone(),
            debug_recorder.clone(),
        )));

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
            debug_recorder,
            process_output_sequence: std::sync::atomic::AtomicU64::new(0),
        };

        // Enable LINE_FEED_NEW_LINE mode (ESC[20h)
        // This makes \n perform both line feed and carriage return
        // which is standard Unix terminal behavior
        terminal.process_output(b"\x1b[20h").ok();

        Ok(terminal)
    }

    pub fn set_debug_recorder(&mut self, recorder: Arc<Mutex<DebugRecorder>>) {
        self.debug_recorder = Some(recorder);
    }

    pub fn process_output(&mut self, data: &[u8]) -> anyhow::Result<()> {
        use std::sync::atomic::Ordering;

        // Get sequence number for this call
        let sequence = self.process_output_sequence.fetch_add(1, Ordering::SeqCst);

        // Store grid before processing
        let grid_before = self.current_grid.clone();

        // Log process output call and PTY output if debug recorder is available
        if let Some(ref recorder) = self.debug_recorder {
            if let Ok(mut rec) = recorder.try_lock() {
                let _ = rec.record_process_output_call(sequence, data);
                let _ = rec.record_pty_output(data);
            }
        }

        // Pass PTY output directly to Alacritty without any transformation
        // Alacritty handles terminal sequences correctly on its own

        // Process the ANSI data through alacritty's parser
        {
            let mut parser = self.parser.lock().unwrap();
            let mut term = self.term.lock().unwrap();

            for byte in data {
                parser.advance(&mut *term, *byte);
            }
        }

        // Update our grid from alacritty's state
        self.sync_grid()?;

        // Log grid changes
        if let Some(ref recorder) = self.debug_recorder {
            if let Ok(mut rec) = recorder.try_lock() {
                let _ = rec.record_grid_before_after(&grid_before, &self.current_grid);
                let _ = rec.record_grid_bottom_context(
                    "server_backend_after_process_output",
                    &self.current_grid,
                    6,
                );
            }
        }

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
                    let _ = writeln!(
                        f,
                        "[{}] sync_grid: Dimension mismatch! Alacritty: {}x{}, Expected: {}x{}",
                        Utc::now().format("%H:%M:%S%.3f"),
                        alac_cols,
                        alac_lines,
                        self.width,
                        self.height
                    );
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
                    let _ = writeln!(
                        f,
                        "[{}] process_output: Failed to resize grid: {}",
                        Utc::now().format("%H:%M:%S%.3f"),
                        e
                    );
                }
            }
        }

        // Clear the grid first (fill with empty cells)
        for row in 0..self.height {
            for col in 0..self.width {
                self.current_grid.set_cell(row, col, Cell::default());
            }
        }

        // Copy all lines from Alacritty's grid (including blank lines in content)
        // We should copy exactly what Alacritty has, not try to remove blank lines
        let lines_to_copy = alac_lines.min(self.height as usize);

        // Log what we're copying and check for the joke content
        if let Some(ref log) = self.debug_log {
            if let Ok(mut f) = log.lock() {
                use std::io::Write;
                let _ = writeln!(
                    f,
                    "[{}] sync_grid: Copying {} lines from Alacritty ({}x{})",
                    Utc::now().format("%H:%M:%S%.3f"),
                    lines_to_copy,
                    alac_cols,
                    alac_lines
                );

                // Debug: Check for joke content in Alacritty's grid
                for line in 0..lines_to_copy.min(50) {
                    let _point = alacritty_terminal::index::Point {
                        line: alacritty_terminal::index::Line(line as i32),
                        column: alacritty_terminal::index::Column(0),
                    };

                    // Get first 60 chars of the line
                    let mut line_text = String::new();
                    for col in 0..60.min(alac_cols) {
                        let point = alacritty_terminal::index::Point {
                            line: alacritty_terminal::index::Line(line as i32),
                            column: alacritty_terminal::index::Column(col),
                        };
                        let cell_ref = &term.grid()[point];
                        line_text.push(cell_ref.c);
                    }

                    let trimmed = line_text.trim_end();
                    if trimmed.contains("programmers")
                        || trimmed.contains("bugs")
                        || trimmed.contains("⏺")
                    {
                        let _ = writeln!(
                            f,
                            "[{}]   Alac Line {}: '{}'",
                            Utc::now().format("%H:%M:%S%.3f"),
                            line,
                            trimmed
                        );
                    } else if !trimmed.is_empty() && line < 10 {
                        // Safely truncate at character boundary
                        let truncated = if trimmed.len() > 40 {
                            let mut end = 40;
                            while !trimmed.is_char_boundary(end) && end > 0 {
                                end -= 1;
                            }
                            &trimmed[..end]
                        } else {
                            trimmed
                        };
                        let _ = writeln!(
                            f,
                            "[{}]   Alac Line {}: '{}'",
                            Utc::now().format("%H:%M:%S%.3f"),
                            line,
                            truncated
                        );
                    }
                }
            }
        }

        // Sync cells from alacritty's grid
        for line in 0..lines_to_copy {
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

        // Debug: Log what we're sending to client
        if let Some(ref log) = self.debug_log {
            if let Ok(mut f) = log.lock() {
                use std::io::Write;
                for row in 0..self.current_grid.height.min(50) {
                    let mut line_text = String::new();
                    for col in 0..self.current_grid.width.min(60) {
                        if let Some(cell) = self.current_grid.get_cell(row, col) {
                            line_text.push(cell.char);
                        }
                    }
                    let trimmed = line_text.trim_end();
                    if trimmed.contains("programmers")
                        || trimmed.contains("bugs")
                        || trimmed.contains("⏺")
                    {
                        let _ = writeln!(
                            f,
                            "[{}]   Grid Line {}: '{}'",
                            Utc::now().format("%H:%M:%S%.3f"),
                            row,
                            trimmed
                        );
                        // Check next few lines for blanks
                        for next_row in (row + 1)..((row + 5).min(self.current_grid.height)) {
                            let mut next_line = String::new();
                            for col in 0..self.current_grid.width.min(60) {
                                if let Some(cell) = self.current_grid.get_cell(next_row, col) {
                                    next_line.push(cell.char);
                                }
                            }
                            if next_line.trim().is_empty() {
                                let _ = writeln!(
                                    f,
                                    "[{}]   Grid Line {}: [BLANK]",
                                    Utc::now().format("%H:%M:%S%.3f"),
                                    next_row
                                );
                            } else {
                                let _ = writeln!(
                                    f,
                                    "[{}]   Grid Line {}: '{}'",
                                    Utc::now().format("%H:%M:%S%.3f"),
                                    next_row,
                                    next_line.trim_end()
                                );
                            }
                        }
                        break;
                    }
                }
            }
        }

        // Sync cursor position (no offset needed since content is at top)
        let cursor_point = term.grid().cursor.point;
        let cursor_row = if cursor_point.line.0 >= 0 && cursor_point.line.0 < lines_to_copy as i32 {
            cursor_point.line.0 as u16
        } else {
            // Cursor is beyond copied content, clamp to last line
            lines_to_copy.saturating_sub(1) as u16
        };
        self.current_grid.cursor = CursorPosition {
            row: cursor_row,
            col: cursor_point.column.0 as u16,
            visible: term
                .mode()
                .contains(alacritty_terminal::term::TermMode::SHOW_CURSOR),
            shape: self.map_cursor_shape(&term),
        };

        // Update timestamp
        self.current_grid.timestamp = Utc::now();

        // Debug: Track absolute line numbers for scrollback
        // Alacritty's history_size gives us how many lines have scrolled off-screen
        let history_size = term.history_size();

        // Log scrollback information
        if let Some(ref recorder) = self.debug_recorder {
            if let Ok(mut rec) = recorder.try_lock() {
                if history_size > 0 {
                    let _ = rec.record_event(crate::debug_recorder::DebugEvent::Comment {
                        timestamp: Utc::now(),
                        text: format!(
                            "AlacrittyBackend: {} lines have scrolled into history",
                            history_size
                        ),
                    });
                }
            }
        }

        // Update line numbers to track absolute position including scrolled content
        let total_lines_processed = history_size as u64 + self.height as u64;
        self.current_grid.start_line =
            crate::server::terminal_state::LineCounter::from_u64(history_size as u64);
        self.current_grid.end_line =
            crate::server::terminal_state::LineCounter::from_u64(total_lines_processed - 1);

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

        // Only add a snapshot when content has scrolled (line numbers changed)
        // This preserves the historical content at different line ranges
        // Check if the line range has changed significantly (scrolling occurred)
        if old_grid.start_line != self.current_grid.start_line
            || old_grid.end_line != self.current_grid.end_line
        {
            // Content has scrolled - preserve this snapshot
            history.add_snapshot(self.current_grid.clone());

            if let Some(ref recorder) = self.debug_recorder {
                if let Ok(mut rec) = recorder.try_lock() {
                    let _ = rec.record_event(crate::debug_recorder::DebugEvent::Comment {
                        timestamp: Utc::now(),
                        text: format!(
                            "AlacrittyBackend: Added snapshot for lines {}-{}",
                            self.current_grid.start_line.to_u64().unwrap_or(0),
                            self.current_grid.end_line.to_u64().unwrap_or(0)
                        ),
                    });
                }
            }
        }

        // Compare Alacritty grid with GridHistory reconstruction
        if let Some(ref recorder) = self.debug_recorder {
            if let Ok(mut rec) = recorder.try_lock() {
                // Get the current GridHistory reconstruction
                if let Ok(reconstructed) = history.get_current() {
                    // Log the comparison
                    let _ = rec.record_alacritty_vs_gridhistory(&self.current_grid, &reconstructed);
                }

                // Also dump the Alacritty grid if it has non-blank content
                let has_content = (0..self.current_grid.height).any(|row| {
                    (0..self.current_grid.width).any(|col| {
                        self.current_grid
                            .get_cell(row, col)
                            .map(|c| c.char != ' ' && c.char != '\0')
                            .unwrap_or(false)
                    })
                });

                if has_content {
                    let _ = rec.record_alacritty_grid_dump(&self.current_grid);
                }
            }
        }

        // Log Alacritty state if debug recorder is available
        if let Some(ref recorder) = self.debug_recorder {
            if let Ok(mut rec) = recorder.try_lock() {
                // Collect sample content - first 10 non-empty lines
                let mut content_sample = Vec::new();
                let mut blank_count = 0;

                for row in 0..self.current_grid.height.min(50) {
                    let mut line_text = String::new();
                    for col in 0..self.current_grid.width.min(80) {
                        if let Some(cell) = self.current_grid.get_cell(row, col) {
                            line_text.push(cell.char);
                        } else {
                            line_text.push(' ');
                        }
                    }
                    let trimmed = line_text.trim_end();
                    if trimmed.is_empty() {
                        blank_count += 1;
                    } else if content_sample.len() < 10 {
                        content_sample.push(format!("Row {}: {}", row, trimmed));
                    }
                }

                let _ = rec.record_alacritty_state(
                    (self.current_grid.width, self.current_grid.height),
                    content_sample,
                    blank_count,
                );
            }
        }

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
            }
            AlacColor::Spec(rgb) => {
                // RGB values are already in 0-255 range in 0.21
                Color::Rgb(rgb.r, rgb.g, rgb.b)
            }
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
                let _ = writeln!(
                    f,
                    "[{}] resize: Requested resize from {}x{} to {}x{}",
                    Utc::now().format("%H:%M:%S%.3f"),
                    self.width,
                    self.height,
                    width,
                    height
                );
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
                    let _ = writeln!(
                        f,
                        "[{}] resize: After resize, alacritty reports: {}x{}",
                        Utc::now().format("%H:%M:%S%.3f"),
                        actual_cols,
                        actual_lines
                    );
                    if actual_cols != width as usize || actual_lines != height as usize {
                        let _ = writeln!(
                            f,
                            "[{}] resize: WARNING! Alacritty did not accept the requested dimensions!",
                            Utc::now().format("%H:%M:%S%.3f")
                        );
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

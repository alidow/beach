use crate::debug_recorder::{DebugEvent, DebugRecorder};
use crate::server::terminal_state::{Grid, GridHistory, LineCounter, TerminalStateError};
use chrono::{DateTime, Utc};
use std::sync::{Arc, Mutex};

pub struct GridView {
    history: Arc<Mutex<GridHistory>>,
    debug_recorder: Option<Arc<Mutex<DebugRecorder>>>,
}

impl GridView {
    pub fn new(history: Arc<Mutex<GridHistory>>) -> Self {
        GridView {
            history,
            debug_recorder: None,
        }
    }

    pub fn set_debug_recorder(&mut self, recorder: Option<Arc<Mutex<DebugRecorder>>>) {
        self.debug_recorder = recorder;
    }

    /// Derive realtime view with optional height limit
    pub fn derive_realtime(&self, max_height: Option<u16>) -> Result<Grid, TerminalStateError> {
        let history = self.history.lock().unwrap();
        let mut grid = history.get_current()?;

        // If height is specified and less than grid height, truncate rows
        if let Some(height) = max_height {
            if height < grid.height {
                grid = self.truncate_to_height(grid, height)?;
            }
        }

        Ok(grid)
    }

    /// Derive view at a specific timestamp with optional height limit
    pub fn derive_at_time(
        &self,
        timestamp: DateTime<Utc>,
        max_height: Option<u16>,
    ) -> Result<Grid, TerminalStateError> {
        let history = self.history.lock().unwrap();
        let mut grid = history.get_at_time(timestamp)?;

        // If height is specified and less than grid height, truncate rows
        if let Some(height) = max_height {
            if height < grid.height {
                grid = self.truncate_to_height(grid, height)?;
            }
        }

        Ok(grid)
    }

    /// Derive view from a specific line number with optional height limit
    /// This now uses actual historical data from GridHistory
    pub fn derive_from_line(
        &self,
        line_num: u64,
        max_height: Option<u16>,
    ) -> Result<Grid, TerminalStateError> {
        // Debug event: HistoricalViewRequested
        if let Some(ref recorder) = self.debug_recorder {
            if let Ok(mut rec) = recorder.try_lock() {
                let _ = rec.record_event(DebugEvent::HistoricalViewRequested {
                    timestamp: Utc::now(),
                    requested_line: line_num,
                    height: max_height.unwrap_or(24),
                });
            }
        }

        let history = self.history.lock().unwrap();

        // Get the historical grid containing the requested line
        let historical_grid = history.get_from_line(line_num)?;

        // Check if the requested line is within the historical grid's range
        let hist_start = historical_grid.start_line.to_u64().unwrap_or(0);
        let hist_end = historical_grid
            .end_line
            .to_u64()
            .unwrap_or(historical_grid.height as u64 - 1);

        // Create a new grid with the requested height (top-anchored from line_num)
        let height = max_height.unwrap_or(historical_grid.height);
        let mut new_grid = Grid::new(historical_grid.width, height);
        new_grid.timestamp = historical_grid.timestamp;

        // Calculate the offset within the historical grid
        let row_offset = if line_num >= hist_start && line_num <= hist_end {
            // Line is within the historical view
            (line_num - hist_start) as u16
        } else if line_num < hist_start {
            // Requested line is before the historical grid (use start of grid)
            0
        } else {
            // Requested line is after the historical grid (shouldn't happen with proper get_from_line)
            // Fall back to showing from the end
            historical_grid.height.saturating_sub(height)
        };

        // Debug: log the copy operation details
        if let Ok(mut debug_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/beach-gridview-debug.log")
        {
            use std::io::Write;
            let _ = writeln!(
                debug_file,
                "[{}] GRIDVIEW_COPY line_num={} hist_range=({},{}) row_offset={} height={}",
                chrono::Utc::now().format("%H:%M:%S%.3f"),
                line_num,
                hist_start,
                hist_end,
                row_offset,
                height
            );

            // Sample the source content being copied
            let mut source_sample = Vec::new();
            let mut source_non_blank = 0;
            for sample_row in row_offset..(row_offset + 3.min(height)) {
                if sample_row < historical_grid.height {
                    let mut line = String::new();
                    for col in 0..historical_grid.width.min(80) {
                        if let Some(cell) = historical_grid.get_cell(sample_row, col) {
                            line.push(cell.char);
                        }
                    }
                    let trimmed = line.trim_end();
                    if !trimmed.is_empty() {
                        source_non_blank += 1;
                    }
                    source_sample.push(format!("src[{}]: '{}'", sample_row, trimmed));
                }
            }
            let _ = writeln!(
                debug_file,
                "[{}] GRIDVIEW_SOURCE: non_blank={}/3 content=[{}]",
                chrono::Utc::now().format("%H:%M:%S%.3f"),
                source_non_blank,
                source_sample.join(", ")
            );
        }

        // Copy the grid content starting from the requested line (top-anchored)
        for dst_row in 0..height {
            let src_row = dst_row + row_offset;
            if src_row < historical_grid.height {
                for col in 0..historical_grid.width {
                    if let Some(cell) = historical_grid.get_cell(src_row, col) {
                        new_grid.set_cell(dst_row, col, cell.clone());
                    }
                }
            }
        }

        // Update line numbers to reflect the actual available view
        // If requested line is before available history, clamp to start of history
        let actual_start = if line_num < hist_start {
            hist_start
        } else {
            line_num
        };
        new_grid.start_line = LineCounter::from_u64(actual_start);
        new_grid.end_line = LineCounter::from_u64(actual_start + height as u64 - 1);

        // Debug event: HistoricalViewReturned
        if let Some(ref recorder) = self.debug_recorder {
            if let Ok(mut rec) = recorder.try_lock() {
                // Collect sample content
                let mut sample_content = Vec::new();
                let mut blank_lines = 0;
                for row in 0..new_grid.height.min(10) {
                    let mut line = String::new();
                    for col in 0..new_grid.width {
                        if let Some(cell) = new_grid.get_cell(row, col) {
                            line.push(cell.char);
                        } else {
                            line.push(' ');
                        }
                    }
                    let trimmed = line.trim_end();
                    if trimmed.is_empty() {
                        blank_lines += 1;
                        sample_content.push(format!("Row {}: [BLANK]", row));
                    } else {
                        sample_content.push(format!("Row {}: {}", row, trimmed));
                    }
                }

                let _ = rec.record_event(DebugEvent::HistoricalViewReturned {
                    timestamp: Utc::now(),
                    requested_line: line_num,
                    returned_start_line: new_grid.start_line.to_u64().unwrap_or(0),
                    returned_end_line: new_grid.end_line.to_u64().unwrap_or(0),
                    blank_lines,
                    sample_content,
                });
            }
        }

        // Adjust cursor position relative to new view
        if historical_grid.cursor.row >= row_offset {
            let cursor_in_view = historical_grid.cursor.row - row_offset;
            if cursor_in_view < height {
                // Cursor is within the new view
                new_grid.cursor = historical_grid.cursor.clone();
                new_grid.cursor.row = cursor_in_view;
            } else {
                // Cursor is below the new view
                new_grid.cursor = historical_grid.cursor.clone();
                new_grid.cursor.visible = false;
            }
        } else {
            // Cursor is above the new view
            new_grid.cursor = historical_grid.cursor.clone();
            new_grid.cursor.visible = false;
        }

        Ok(new_grid)
    }

    /// Truncate grid to specified number of rows
    /// Bottom-aligns the view: keeps the last `height` rows of the grid.
    fn truncate_to_height(&self, grid: Grid, height: u16) -> Result<Grid, TerminalStateError> {
        let mut new_grid = Grid::new(grid.width, height);
        new_grid.timestamp = grid.timestamp;

        // Determine starting source row to keep the bottom `height` rows
        let total = grid.height as usize;
        let keep = height as usize;
        let src_start = if total > keep { total - keep } else { 0 } as u16;

        // Copy rows from src_start.. into destination 0..
        for (dst_row, src_row) in (0..height).zip(src_start..grid.height) {
            for col in 0..grid.width {
                if let Some(cell) = grid.get_cell(src_row, col) {
                    new_grid.set_cell(dst_row, col, cell.clone());
                }
            }
        }

        // Preserve cursor position (even if beyond visible area)
        // Note: we keep the same semantics as before; cursor may be outside the visible window
        new_grid.cursor = grid.cursor.clone();

        // Adjust line numbers to reflect the bottom-aligned slice
        let end_line_u64 = grid
            .end_line
            .to_u64()
            .unwrap_or_else(|| grid.height as u64 - 1);
        let start_line_u64 = end_line_u64.saturating_sub(height as u64 - 1);
        new_grid.start_line = LineCounter::from_u64(start_line_u64);
        new_grid.end_line = LineCounter::from_u64(end_line_u64);

        Ok(new_grid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::terminal_state::{
        Cell, CellAttributes, Color, Grid, GridDelta, GridHistory,
    };
    use std::sync::{Arc, Mutex};

    fn create_test_history() -> Arc<Mutex<GridHistory>> {
        let mut grid = Grid::new(80, 24);
        grid.timestamp = chrono::Utc::now();
        Arc::new(Mutex::new(GridHistory::new(grid)))
    }

    fn create_test_history_with_content(
        width: u16,
        height: u16,
        lines: Vec<&str>,
    ) -> Arc<Mutex<GridHistory>> {
        let initial = Grid::new(width, height);
        let history = Arc::new(Mutex::new(GridHistory::new(initial)));

        let grid = create_grid_with_text(width, height, lines);

        {
            let mut hist = history.lock().unwrap();
            // Get current state for diffing
            let current = hist
                .get_current()
                .unwrap_or_else(|_| Grid::new(width, height));
            let delta = GridDelta::diff(&current, &grid);
            hist.add_delta(delta);
        }

        history
    }

    fn create_grid_with_text(width: u16, height: u16, lines: Vec<&str>) -> Grid {
        let mut grid = Grid::new(width, height);

        for (row_idx, line) in lines.iter().enumerate() {
            for (col_idx, ch) in line.chars().enumerate() {
                if row_idx < height as usize && col_idx < width as usize {
                    grid.set_cell(
                        row_idx as u16,
                        col_idx as u16,
                        Cell {
                            char: ch,
                            fg_color: Color::Default,
                            bg_color: Color::Default,
                            attributes: CellAttributes::default(),
                        },
                    );
                }
            }
        }

        grid
    }

    #[test_timeout::timeout]
    fn test_derive_realtime_no_height() {
        // Test that without height parameter, full grid is returned
        let history = create_test_history_with_content(
            80,
            24,
            vec!["First line", "Second line", "Third line"],
        );
        let view = GridView::new(history.clone());

        let result = view.derive_realtime(None).unwrap();
        assert_eq!(result.width, 80);
        assert_eq!(result.height, 24);

        // Check that content is preserved
        let mut first_line = String::new();
        for col in 0..11 {
            if let Some(cell) = result.get_cell(0, col) {
                first_line.push(cell.char);
            }
        }
        assert_eq!(first_line, "First line ");
    }

    #[test_timeout::timeout]
    fn test_derive_realtime_with_height_truncation() {
        // Test that height parameter truncates grid
        let history = create_test_history_with_content(
            80,
            24,
            vec!["Line 1", "Line 2", "Line 3", "Line 4", "Line 5"],
        );
        let view = GridView::new(history.clone());

        // Request only 3 rows
        let result = view.derive_realtime(Some(3)).unwrap();
        assert_eq!(result.width, 80); // Width unchanged
        assert_eq!(result.height, 3); // Height truncated

        // Check first 3 lines are preserved
        for row in 0..3 {
            let mut line = String::new();
            for col in 0..6 {
                if let Some(cell) = result.get_cell(row, col) {
                    line.push(cell.char);
                }
            }
            assert_eq!(line, format!("Line {}", row + 1));
        }
    }

    #[test_timeout::timeout]
    fn test_derive_realtime_height_larger_than_grid() {
        // Test that requesting more height than available returns full grid
        let history = create_test_history_with_content(80, 10, vec!["Line 1", "Line 2"]);
        let view = GridView::new(history.clone());

        // Request 20 rows but grid only has 10
        let result = view.derive_realtime(Some(20)).unwrap();
        assert_eq!(result.width, 80);
        assert_eq!(result.height, 10); // Original height preserved
    }

    #[test_timeout::timeout]
    fn test_truncate_preserves_cursor_position() {
        // Test that cursor position is preserved even if beyond visible area
        let initial = Grid::new(80, 24);
        let history = Arc::new(Mutex::new(GridHistory::new(initial)));

        let mut grid = create_grid_with_text(80, 24, vec!["Line 1", "Line 2", "Line 3", "Line 4"]);

        // Set cursor to row 10
        grid.cursor.row = 10;
        grid.cursor.col = 5;
        grid.cursor.visible = true;

        {
            let mut hist = history.lock().unwrap();
            let current = hist.get_current().unwrap_or_else(|_| Grid::new(80, 24));
            let delta = GridDelta::diff(&current, &grid);
            hist.add_delta(delta);
        }

        let view = GridView::new(history.clone());

        // Truncate to 5 rows
        let result = view.derive_realtime(Some(5)).unwrap();
        assert_eq!(result.height, 5);

        // Cursor position should be preserved even though it's beyond visible area
        assert_eq!(result.cursor.row, 10);
        assert_eq!(result.cursor.col, 5);
        assert_eq!(result.cursor.visible, true);
    }

    #[test_timeout::timeout]
    fn test_truncate_preserves_full_width() {
        // Test that width is always preserved, including trailing spaces
        let history = create_test_history_with_content(
            100,
            24,
            vec![
                "Short line with lots of trailing spaces                                                            ",
                "Another line                                                                                        ",
            ],
        );
        let view = GridView::new(history.clone());

        let result = view.derive_realtime(Some(2)).unwrap();
        assert_eq!(result.width, 100); // Full width preserved
        assert_eq!(result.height, 2);

        // Check that trailing spaces are preserved
        let mut line_length = 0;
        for col in 0..100 {
            if let Some(_cell) = result.get_cell(0, col) {
                line_length = col + 1;
            }
        }
        assert_eq!(line_length, 100); // All columns should have cells (even if spaces)
    }

    #[test_timeout::timeout]
    fn test_preserves_colors_and_attributes() {
        // Test that colors and attributes are preserved
        let initial = Grid::new(80, 24);
        let history = Arc::new(Mutex::new(GridHistory::new(initial)));

        let mut grid = Grid::new(80, 24);

        // Create colored text
        let text = "Colored";
        for (idx, ch) in text.chars().enumerate() {
            grid.set_cell(
                0,
                idx as u16,
                Cell {
                    char: ch,
                    fg_color: Color::Rgb(255, 0, 0),
                    bg_color: Color::Indexed(4),
                    attributes: CellAttributes {
                        bold: true,
                        italic: false,
                        underline: true,
                        reverse: false,
                        blink: false,
                        strikethrough: false,
                        dim: false,
                        hidden: false,
                    },
                },
            );
        }

        {
            let mut hist = history.lock().unwrap();
            let current = hist.get_current().unwrap_or_else(|_| Grid::new(80, 24));
            let delta = GridDelta::diff(&current, &grid);
            hist.add_delta(delta);
        }

        let view = GridView::new(history.clone());
        let result = view.derive_realtime(Some(10)).unwrap();

        // Check first cell preserved colors
        if let Some(cell) = result.get_cell(0, 0) {
            assert_eq!(cell.char, 'C');
            match &cell.fg_color {
                Color::Rgb(r, g, b) => {
                    assert_eq!(*r, 255);
                    assert_eq!(*g, 0);
                    assert_eq!(*b, 0);
                }
                _ => panic!("Expected RGB color"),
            }
            assert_eq!(cell.bg_color, Color::Indexed(4));
            assert!(cell.attributes.bold);
            assert!(cell.attributes.underline);
        }
    }
}

use std::sync::{Arc, Mutex};
use chrono::{DateTime, Utc};
use crate::server::terminal_state::{Grid, GridHistory, TerminalStateError, LineCounter};

pub struct GridView {
    history: Arc<Mutex<GridHistory>>,
}

impl GridView {
    pub fn new(history: Arc<Mutex<GridHistory>>) -> Self {
        GridView { history }
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
    pub fn derive_at_time(&self, timestamp: DateTime<Utc>, max_height: Option<u16>) -> Result<Grid, TerminalStateError> {
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
    pub fn derive_from_line(&self, line_num: u64, max_height: Option<u16>) -> Result<Grid, TerminalStateError> {
        let history = self.history.lock().unwrap();
        
        // Get the current grid (most recent state)
        let current_grid = history.get_current()?;
        
        // Check if the requested line is within the current terminal's range
        let current_start = current_grid.start_line.to_u64().unwrap_or(0);
        let current_end = current_grid.end_line.to_u64().unwrap_or(current_grid.height as u64 - 1);
        
        // If line_num is beyond the end, just return the current view
        if line_num > current_end {
            // Return the current view (can't scroll beyond what exists)
            let mut grid = current_grid;
            if let Some(height) = max_height {
                if height < grid.height {
                    grid = self.truncate_to_height(grid, height)?;
                }
            }
            return Ok(grid);
        }
        
        // Create a new grid starting from the requested line
        let height = max_height.unwrap_or(current_grid.height);
        let mut new_grid = Grid::new(current_grid.width, height);
        new_grid.timestamp = current_grid.timestamp;
        
        // Calculate the offset to shift the view
        let row_offset = if line_num >= current_start && line_num <= current_end {
            // Line is within current view, shift accordingly
            (line_num - current_start) as u16
        } else if line_num < current_start {
            // Line is before current view (shouldn't happen with current terminal model)
            0
        } else {
            // Line is after current view (already handled above)
            0
        };
        
        // Copy the grid content starting from the requested line
        for dst_row in 0..height {
            let src_row = dst_row + row_offset;
            if src_row < current_grid.height {
                for col in 0..current_grid.width {
                    if let Some(cell) = current_grid.get_cell(src_row, col) {
                        new_grid.set_cell(dst_row, col, cell.clone());
                    }
                }
            }
        }
        
        // Update line numbers
        new_grid.start_line = LineCounter::from_u64(line_num);
        new_grid.end_line = LineCounter::from_u64(line_num + height as u64 - 1);
        
        // Adjust cursor position relative to new view
        if current_grid.cursor.row >= row_offset {
            new_grid.cursor = current_grid.cursor.clone();
            new_grid.cursor.row -= row_offset;
        } else {
            // Cursor is above the new view
            new_grid.cursor = current_grid.cursor.clone();
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
        let end_line_u64 = grid.end_line.to_u64().unwrap_or_else(|| grid.height as u64 - 1);
        let start_line_u64 = end_line_u64.saturating_sub(height as u64 - 1);
        new_grid.start_line = LineCounter::from_u64(start_line_u64);
        new_grid.end_line = LineCounter::from_u64(end_line_u64);

        Ok(new_grid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use crate::server::terminal_state::{Grid, GridHistory, GridDelta, Cell, Color, CellAttributes};
    
    fn create_test_history() -> Arc<Mutex<GridHistory>> {
        let mut grid = Grid::new(80, 24);
        grid.timestamp = chrono::Utc::now();
        Arc::new(Mutex::new(GridHistory::new(grid)))
    }
    
    fn create_test_history_with_content(width: u16, height: u16, lines: Vec<&str>) -> Arc<Mutex<GridHistory>> {
        let initial = Grid::new(width, height);
        let history = Arc::new(Mutex::new(GridHistory::new(initial)));
        
        let grid = create_grid_with_text(width, height, lines);
        
        {
            let mut hist = history.lock().unwrap();
            // Get current state for diffing
            let current = hist.get_current().unwrap_or_else(|_| Grid::new(width, height));
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
                    grid.set_cell(row_idx as u16, col_idx as u16, Cell {
                        char: ch,
                        fg_color: Color::Default,
                        bg_color: Color::Default,
                        attributes: CellAttributes::default(),
                    });
                }
            }
        }
        
        grid
    }
    
    #[test]
    fn test_derive_realtime_no_height() {
        // Test that without height parameter, full grid is returned
        let history = create_test_history_with_content(80, 24, vec![
            "First line",
            "Second line",
            "Third line",
        ]);
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
    
    #[test]
    fn test_derive_realtime_with_height_truncation() {
        // Test that height parameter truncates grid
        let history = create_test_history_with_content(80, 24, vec![
            "Line 1",
            "Line 2",
            "Line 3",
            "Line 4",
            "Line 5",
        ]);
        let view = GridView::new(history.clone());
        
        // Request only 3 rows
        let result = view.derive_realtime(Some(3)).unwrap();
        assert_eq!(result.width, 80); // Width unchanged
        assert_eq!(result.height, 3);  // Height truncated
        
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
    
    #[test]
    fn test_derive_realtime_height_larger_than_grid() {
        // Test that requesting more height than available returns full grid
        let history = create_test_history_with_content(80, 10, vec![
            "Line 1",
            "Line 2",
        ]);
        let view = GridView::new(history.clone());
        
        // Request 20 rows but grid only has 10
        let result = view.derive_realtime(Some(20)).unwrap();
        assert_eq!(result.width, 80);
        assert_eq!(result.height, 10); // Original height preserved
    }
    
    #[test]
    fn test_truncate_preserves_cursor_position() {
        // Test that cursor position is preserved even if beyond visible area
        let initial = Grid::new(80, 24);
        let history = Arc::new(Mutex::new(GridHistory::new(initial)));
        
        let mut grid = create_grid_with_text(80, 24, vec![
            "Line 1",
            "Line 2",
            "Line 3",
            "Line 4",
        ]);
        
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
    
    #[test]
    fn test_truncate_preserves_full_width() {
        // Test that width is always preserved, including trailing spaces
        let history = create_test_history_with_content(100, 24, vec![
            "Short line with lots of trailing spaces                                                            ",
            "Another line                                                                                        ",
        ]);
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
    
    #[test]
    fn test_preserves_colors_and_attributes() {
        // Test that colors and attributes are preserved
        let initial = Grid::new(80, 24);
        let history = Arc::new(Mutex::new(GridHistory::new(initial)));
        
        let mut grid = Grid::new(80, 24);
        
        // Create colored text
        let text = "Colored";
        for (idx, ch) in text.chars().enumerate() {
            grid.set_cell(0, idx as u16, Cell {
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
            });
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
                },
                _ => panic!("Expected RGB color"),
            }
            assert_eq!(cell.bg_color, Color::Indexed(4));
            assert!(cell.attributes.bold);
            assert!(cell.attributes.underline);
        }
    }
}

use std::sync::{Arc, Mutex};
use crate::server::terminal_state::{Grid, GridHistory, TerminalStateError};

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
    
    /// Truncate grid to specified number of rows
    fn truncate_to_height(&self, grid: Grid, height: u16) -> Result<Grid, TerminalStateError> {
        let mut new_grid = Grid::new(grid.width, height);
        new_grid.timestamp = grid.timestamp;
        
        // Copy only the first 'height' rows, preserving full width
        for row in 0..height {
            for col in 0..grid.width {
                if let Some(cell) = grid.get_cell(row, col) {
                    new_grid.set_cell(row, col, cell.clone());
                }
            }
        }
        
        // Preserve cursor position (even if beyond visible area)
        new_grid.cursor = grid.cursor.clone();
        
        new_grid.start_line = grid.start_line;
        new_grid.end_line = grid.end_line;
        
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
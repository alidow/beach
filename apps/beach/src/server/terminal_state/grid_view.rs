use std::sync::{Arc, Mutex};
use crate::server::terminal_state::{Grid, GridHistory, TerminalStateError};

pub struct GridView {
    history: Arc<Mutex<GridHistory>>,
}

impl GridView {
    pub fn new(history: Arc<Mutex<GridHistory>>) -> Self {
        GridView { history }
    }
    
    /// Derive realtime view with optional dimensions
    pub fn derive_realtime(&self, dimensions: Option<(u16, u16)>) -> Result<Grid, TerminalStateError> {
        let history = self.history.lock().unwrap();
        let mut grid = history.get_current()?;
        
        if let Some((width, height)) = dimensions {
            grid = self.rewrap_grid(grid, width, height)?;
        }
        
        Ok(grid)
    }
    
    /// Simple truncation for very large grids
    fn simple_truncate_grid(&self, grid: Grid, new_width: u16, new_height: u16) -> Result<Grid, TerminalStateError> {
        let mut new_grid = Grid::new(new_width, new_height);
        new_grid.timestamp = grid.timestamp;
        
        // Copy cells directly, truncating to new dimensions
        let copy_height = grid.height.min(new_height);
        let copy_width = grid.width.min(new_width);
        
        for row in 0..copy_height {
            for col in 0..copy_width {
                if let Some(cell) = grid.get_cell(row, col) {
                    new_grid.set_cell(row, col, cell.clone());
                }
            }
        }
        
        // Adjust cursor if needed
        new_grid.cursor.row = grid.cursor.row.min(new_height.saturating_sub(1));
        new_grid.cursor.col = grid.cursor.col.min(new_width.saturating_sub(1));
        new_grid.cursor.visible = grid.cursor.visible;
        
        new_grid.start_line = grid.start_line;
        new_grid.end_line = grid.end_line;
        
        Ok(new_grid)
    }
    
    /// Re-wrap grid content to new dimensions
    fn rewrap_grid(&self, grid: Grid, new_width: u16, new_height: u16) -> Result<Grid, TerminalStateError> {
        // For very large grids, just do a simple truncation to avoid performance issues
        // TODO: Implement proper wrapping for reasonable sized grids
        if grid.width > 500 || grid.height > 500 {
            return self.simple_truncate_grid(grid, new_width, new_height);
        }
        
        let mut new_grid = Grid::new(new_width, new_height);
        new_grid.timestamp = grid.timestamp;
        
        // Extract all text from the original grid, but limit to a reasonable number of rows
        let mut all_lines = Vec::new();
        let max_rows = grid.height.min(200); // Process at most 200 rows
        
        for row in 0..max_rows {
            let mut line = String::new();
            let mut last_non_empty_col = -1i32;
            
            // Collect all characters in this row
            for col in 0..grid.width {
                if let Some(cell) = grid.get_cell(row, col) {
                    if cell.char != '\0' {
                        line.push(cell.char);
                        if cell.char != ' ' {
                            last_non_empty_col = line.len() as i32 - 1;
                        }
                    }
                }
            }
            
            // Trim trailing spaces but keep the line
            if last_non_empty_col >= 0 {
                line.truncate((last_non_empty_col + 1) as usize);
            } else {
                line.clear(); // All spaces or empty
            }
            all_lines.push(line);
        }
        
        // Re-wrap the text into the new width
        let mut wrapped_lines = Vec::new();
        for line in all_lines.iter() {
            if line.is_empty() {
                wrapped_lines.push(String::new());
            } else if line.len() <= new_width as usize {
                wrapped_lines.push(line.clone());
            } else {
                // Wrap long lines
                let mut remaining = &line[..];
                while !remaining.is_empty() {
                    let chunk_len = (new_width as usize).min(remaining.len());
                    wrapped_lines.push(remaining[..chunk_len].to_string());
                    remaining = &remaining[chunk_len..];
                }
            }
        }
        
        // Fill the new grid with wrapped text
        for (row_idx, line) in wrapped_lines.iter().enumerate() {
            if row_idx >= new_height as usize {
                break;
            }
            
            for (col_idx, ch) in line.chars().enumerate() {
                if col_idx >= new_width as usize {
                    break;
                }
                
                // Try to preserve attributes from original if possible
                let orig_row = row_idx.min(grid.height as usize - 1);
                let orig_col = col_idx.min(grid.width as usize - 1);
                
                if let Some(orig_cell) = grid.get_cell(orig_row as u16, orig_col as u16) {
                    let mut cell = orig_cell.clone();
                    cell.char = ch;
                    new_grid.set_cell(row_idx as u16, col_idx as u16, cell);
                } else {
                    // Create default cell
                    use crate::server::terminal_state::{Cell, Color, CellAttributes};
                    new_grid.set_cell(row_idx as u16, col_idx as u16, Cell {
                        char: ch,
                        fg_color: Color::Default,
                        bg_color: Color::Default,
                        attributes: CellAttributes::default(),
                    });
                }
            }
        }
        
        // Update cursor position if it's beyond the new width
        if grid.cursor.col >= new_width {
            // Calculate wrapped position
            let linear_pos = (grid.cursor.row as usize * grid.width as usize) + grid.cursor.col as usize;
            let new_row = (linear_pos / new_width as usize) as u16;
            let new_col = (linear_pos % new_width as usize) as u16;
            new_grid.cursor.row = new_row.min(new_height - 1);
            new_grid.cursor.col = new_col;
        } else {
            new_grid.cursor = grid.cursor.clone();
        }
        
        new_grid.start_line = grid.start_line.clone();
        new_grid.end_line = grid.end_line.clone();
        
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
    fn test_derive_realtime_no_dimensions() {
        // Test deriving realtime view without dimension changes
        let history = create_test_history_with_content(80, 24, vec![
            "Hello World",
            "Second Line",
            "Third Line",
        ]);
        
        let view = GridView::new(history.clone());
        let result = view.derive_realtime(None).unwrap();
        
        assert_eq!(result.width, 80);
        assert_eq!(result.height, 24);
        
        // Check content
        let mut line = String::new();
        for col in 0..11 {
            if let Some(cell) = result.get_cell(0, col) {
                line.push(cell.char);
            }
        }
        assert_eq!(line.trim(), "Hello World");
    }
    
    #[test]
    fn test_derive_realtime_with_dimensions() {
        // Test deriving realtime view with dimension changes
        let history = create_test_history_with_content(80, 24, vec![
            "This is a very long line that will wrap when we reduce the width",
        ]);
        
        let view = GridView::new(history.clone());
        
        // Test with smaller width
        let result = view.derive_realtime(Some((30, 24))).unwrap();
        
        assert_eq!(result.width, 30);
        assert_eq!(result.height, 24);
        
        // Check that text wrapped to multiple lines
        let mut first_line = String::new();
        for col in 0..30 {
            if let Some(cell) = result.get_cell(0, col) {
                if cell.char != '\0' {
                    first_line.push(cell.char);
                }
            }
        }
        // First line should contain part of the text
        assert!(first_line.trim().len() > 0);
        assert!(first_line.trim().len() <= 30);
        
        // Check second line has continuation
        let mut second_line = String::new();
        for col in 0..30 {
            if let Some(cell) = result.get_cell(1, col) {
                if cell.char != '\0' && cell.char != ' ' {
                    second_line.push(cell.char);
                }
            }
        }
        assert!(!second_line.trim().is_empty());
    }
    
    #[test]
    fn test_rewrap_grid_basic() {
        // Test basic text wrapping
        let history = create_test_history();
        let view = GridView::new(history.clone());
        
        let grid = create_grid_with_text(80, 24, vec![
            "The quick brown fox jumps over the lazy dog",
            "Second line here",
        ]);
        
        let wrapped = view.rewrap_grid(grid, 20, 10).unwrap();
        
        assert_eq!(wrapped.width, 20);
        assert_eq!(wrapped.height, 10);
        
        // First line should wrap to multiple lines
        let mut line1 = String::new();
        for col in 0..20 {
            if let Some(cell) = wrapped.get_cell(0, col) {
                if cell.char != '\0' {
                    line1.push(cell.char);
                }
            }
        }
        assert_eq!(line1.trim(), "The quick brown fox");
        
        let mut line2 = String::new();
        for col in 0..20 {
            if let Some(cell) = wrapped.get_cell(1, col) {
                if cell.char != '\0' {
                    line2.push(cell.char);
                }
            }
        }
        assert_eq!(line2.trim(), "jumps over the lazy");
    }
    
    #[test]
    fn test_rewrap_grid_wider() {
        // Test expanding to wider grid (no wrapping needed)
        let history = create_test_history();
        let view = GridView::new(history.clone());
        
        let grid = create_grid_with_text(40, 24, vec![
            "Short line",
            "Another one",
        ]);
        
        let wrapped = view.rewrap_grid(grid, 100, 24).unwrap();
        
        assert_eq!(wrapped.width, 100);
        assert_eq!(wrapped.height, 24);
        
        // Lines should remain on same rows
        let mut line1 = String::new();
        for col in 0..100 {
            if let Some(cell) = wrapped.get_cell(0, col) {
                if cell.char != '\0' && cell.char != ' ' {
                    line1.push(cell.char);
                }
            }
        }
        assert_eq!(line1, "Shortline");
    }
    
    #[test]
    fn test_rewrap_preserves_colors() {
        // Test that colors and attributes are preserved during rewrap
        let history = create_test_history();
        let view = GridView::new(history.clone());
        
        let mut grid = Grid::new(80, 24);
        
        // Create colored text
        let text = "Colored text here";
        for (idx, ch) in text.chars().enumerate() {
            grid.set_cell(0, idx as u16, Cell {
                char: ch,
                fg_color: Color::Rgb(255, 0, 0),
                bg_color: Color::Indexed(4),
                attributes: CellAttributes {
                    bold: true,
                    italic: false,
                    underline: true,
                    ..Default::default()
                },
            });
        }
        
        let wrapped = view.rewrap_grid(grid, 10, 24).unwrap();
        
        // Check first character still has colors
        if let Some(cell) = wrapped.get_cell(0, 0) {
            assert_eq!(cell.fg_color, Color::Rgb(255, 0, 0));
            assert_eq!(cell.bg_color, Color::Indexed(4));
            assert!(cell.attributes.bold);
            assert!(cell.attributes.underline);
        }
    }
    
    #[test]
    fn test_rewrap_empty_lines() {
        // Test handling of empty lines
        let history = create_test_history();
        let view = GridView::new(history.clone());
        
        let grid = create_grid_with_text(80, 24, vec![
            "First line",
            "",
            "Third line",
            "",
            "Fifth line",
        ]);
        
        let wrapped = view.rewrap_grid(grid, 40, 24).unwrap();
        
        // Empty lines should be preserved
        let mut line2 = String::new();
        for col in 0..40 {
            if let Some(cell) = wrapped.get_cell(1, col) {
                if cell.char != '\0' && cell.char != ' ' {
                    line2.push(cell.char);
                }
            }
        }
        assert!(line2.is_empty());
        
        let mut line3 = String::new();
        for col in 0..40 {
            if let Some(cell) = wrapped.get_cell(2, col) {
                if cell.char != '\0' && cell.char != ' ' {
                    line3.push(cell.char);
                }
            }
        }
        assert_eq!(line3, "Thirdline");
    }
    
    #[test]
    fn test_rewrap_very_long_line() {
        // Test wrapping a very long line
        let history = create_test_history();
        let view = GridView::new(history.clone());
        
        let long_text = "A".repeat(200);
        let grid = create_grid_with_text(250, 24, vec![&long_text]);
        
        let wrapped = view.rewrap_grid(grid, 50, 24).unwrap();
        
        // Should wrap to exactly 4 lines (200 / 50 = 4)
        for row in 0..4 {
            let mut line = String::new();
            for col in 0..50 {
                if let Some(cell) = wrapped.get_cell(row, col) {
                    if cell.char != '\0' {
                        line.push(cell.char);
                    }
                }
            }
            assert_eq!(line.len(), 50);
            assert!(line.chars().all(|c| c == 'A'));
        }
    }
    
    #[test]
    fn test_rewrap_cursor_position() {
        // Test cursor position adjustment during rewrap
        let history = create_test_history();
        let view = GridView::new(history.clone());
        
        let mut grid = create_grid_with_text(80, 24, vec![
            "Some text here",
        ]);
        grid.cursor.row = 0;
        grid.cursor.col = 70; // Beyond new width
        grid.cursor.visible = true;
        
        let wrapped = view.rewrap_grid(grid, 20, 24).unwrap();
        
        // Cursor should be adjusted to new position
        assert!(wrapped.cursor.col < 20);
        assert_eq!(wrapped.cursor.visible, true);
    }
    
    #[test]
    fn test_simple_truncate_for_large_grids() {
        // Test that very large grids use simple truncation
        let history = create_test_history();
        let view = GridView::new(history.clone());
        
        let mut grid = Grid::new(600, 600); // Large grid
        grid.set_cell(0, 0, Cell {
            char: 'X',
            fg_color: Color::Default,
            bg_color: Color::Default,
            attributes: CellAttributes::default(),
        });
        
        let result = view.rewrap_grid(grid, 100, 100).unwrap();
        
        assert_eq!(result.width, 100);
        assert_eq!(result.height, 100);
        
        // Should have truncated, preserving top-left content
        if let Some(cell) = result.get_cell(0, 0) {
            assert_eq!(cell.char, 'X');
        }
    }
    
    #[test]
    fn test_unicode_handling() {
        // Test handling of unicode characters
        let history = create_test_history();
        let view = GridView::new(history.clone());
        
        let grid = create_grid_with_text(80, 24, vec![
            "Hello 世界 🦀 Rust",
            "Émojis: 🏖️ 🚀 🎉",
        ]);
        
        let wrapped = view.rewrap_grid(grid, 40, 24).unwrap();
        
        // Unicode should be preserved
        let mut line1 = String::new();
        for col in 0..40 {
            if let Some(cell) = wrapped.get_cell(0, col) {
                if cell.char != '\0' {
                    line1.push(cell.char);
                }
            }
        }
        assert!(line1.contains('世'));
        assert!(line1.contains('🦀'));
    }
}

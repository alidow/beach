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

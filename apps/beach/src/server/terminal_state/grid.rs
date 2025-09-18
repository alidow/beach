use crate::server::terminal_state::{Cell, LineCounter};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Grid {
    /// Grid dimensions at capture time
    pub width: u16,
    pub height: u16,

    /// 2D array of cells [row][col]
    pub cells: Vec<Vec<Cell>>,

    /// Line number at top of grid
    pub start_line: LineCounter,

    /// Line number at bottom of grid
    pub end_line: LineCounter,

    /// Cursor position (may be hidden)
    pub cursor: CursorPosition,

    /// Timestamp when grid was captured
    pub timestamp: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CursorPosition {
    pub row: u16,
    pub col: u16,
    pub visible: bool,
    pub shape: CursorShape,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum CursorShape {
    Block,
    Underline,
    Bar,
}

impl Grid {
    pub fn new(width: u16, height: u16) -> Self {
        Grid {
            width,
            height,
            cells: vec![vec![Cell::default(); width as usize]; height as usize],
            start_line: LineCounter::new(),
            end_line: LineCounter::from_u64(if height > 0 { height as u64 - 1 } else { 0 }),
            cursor: CursorPosition {
                row: 0,
                col: 0,
                visible: true,
                shape: CursorShape::Block,
            },
            timestamp: Utc::now(),
        }
    }

    /// Get cell at position
    pub fn get_cell(&self, row: u16, col: u16) -> Option<&Cell> {
        self.cells.get(row as usize)?.get(col as usize)
    }

    /// Get mutable reference to cell at position
    pub fn get_cell_mut(&mut self, row: u16, col: u16) -> Option<&mut Cell> {
        self.cells.get_mut(row as usize)?.get_mut(col as usize)
    }

    /// Set cell at position
    pub fn set_cell(&mut self, row: u16, col: u16, cell: Cell) {
        if let Some(row_cells) = self.cells.get_mut(row as usize) {
            if let Some(target_cell) = row_cells.get_mut(col as usize) {
                *target_cell = cell;
            }
        }
    }

    /// Count the number of blank lines in the grid
    pub fn count_blank_lines(&self) -> usize {
        self.cells
            .iter()
            .filter(|row| row.iter().all(|cell| cell.is_blank()))
            .count()
    }

    /// Get content distribution - returns a list of (row_index, has_content) pairs
    /// to show where content vs blank lines are
    pub fn get_content_distribution(&self) -> Vec<(u16, bool)> {
        self.cells
            .iter()
            .enumerate()
            .map(|(row_idx, row)| {
                let has_content = !row.iter().all(|cell| cell.is_blank());
                (row_idx as u16, has_content)
            })
            .collect()
    }

    /// Resize grid to new dimensions
    pub fn resize(
        &mut self,
        new_width: u16,
        new_height: u16,
    ) -> Result<(), crate::server::terminal_state::TerminalStateError> {
        if new_width == 0 || new_height == 0 {
            return Err(
                crate::server::terminal_state::TerminalStateError::InvalidDimensions {
                    width: new_width,
                    height: new_height,
                },
            );
        }

        // Resize rows
        self.cells.resize(
            new_height as usize,
            vec![Cell::default(); new_width as usize],
        );

        // Resize columns in each row
        for row in &mut self.cells {
            row.resize(new_width as usize, Cell::default());
        }

        self.width = new_width;
        self.height = new_height;

        // Adjust cursor if out of bounds
        if self.cursor.row >= new_height {
            self.cursor.row = if new_height > 0 { new_height - 1 } else { 0 };
        }
        if self.cursor.col >= new_width {
            self.cursor.col = if new_width > 0 { new_width - 1 } else { 0 };
        }

        Ok(())
    }
}

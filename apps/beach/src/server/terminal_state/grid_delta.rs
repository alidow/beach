use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::server::terminal_state::{Cell, CursorPosition, Grid};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GridDelta {
    /// Timestamp of change
    pub timestamp: DateTime<Utc>,
    
    /// Changed cells (sparse representation)
    pub cell_changes: Vec<CellChange>,
    
    /// Dimension change if any
    pub dimension_change: Option<DimensionChange>,
    
    /// Cursor movement
    pub cursor_change: Option<CursorChange>,
    
    /// Sequence number for ordering
    pub sequence: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CellChange {
    pub row: u16,
    pub col: u16,
    pub old_cell: Cell,
    pub new_cell: Cell,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DimensionChange {
    pub old_width: u16,
    pub old_height: u16,
    pub new_width: u16,
    pub new_height: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CursorChange {
    pub old_position: CursorPosition,
    pub new_position: CursorPosition,
}

impl GridDelta {
    /// Create minimal delta between two grids
    pub fn diff(old: &Grid, new: &Grid) -> Self {
        let mut cell_changes = Vec::new();
        
        // Only track actual changes
        for row in 0..old.height.min(new.height) {
            for col in 0..old.width.min(new.width) {
                let old_cell = old.get_cell(row, col);
                let new_cell = new.get_cell(row, col);
                
                if old_cell != new_cell {
                    if let (Some(old), Some(new)) = (old_cell, new_cell) {
                        cell_changes.push(CellChange {
                            row,
                            col,
                            old_cell: old.clone(),
                            new_cell: new.clone(),
                        });
                    }
                }
            }
        }
        
        // Check dimension changes
        let dimension_change = if old.width != new.width || old.height != new.height {
            Some(DimensionChange {
                old_width: old.width,
                old_height: old.height,
                new_width: new.width,
                new_height: new.height,
            })
        } else {
            None
        };
        
        // Check cursor changes
        let cursor_change = if old.cursor != new.cursor {
            Some(CursorChange {
                old_position: old.cursor.clone(),
                new_position: new.cursor.clone(),
            })
        } else {
            None
        };
        
        GridDelta {
            timestamp: new.timestamp,
            cell_changes,
            dimension_change,
            cursor_change,
            sequence: 0, // Set by history manager
        }
    }
    
    /// Apply delta to grid
    pub fn apply(&self, grid: &mut Grid) -> Result<(), crate::server::terminal_state::TerminalStateError> {
        // Apply dimension changes first
        if let Some(dim_change) = &self.dimension_change {
            grid.resize(dim_change.new_width, dim_change.new_height)?;
        }
        
        // Apply cell changes
        for change in &self.cell_changes {
            grid.set_cell(change.row, change.col, change.new_cell.clone());
        }
        
        // Apply cursor changes
        if let Some(cursor_change) = &self.cursor_change {
            grid.cursor = cursor_change.new_position.clone();
        }
        
        Ok(())
    }
}

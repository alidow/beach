use std::sync::{Arc, Mutex};
use crate::server::terminal_state::{
    Grid, GridHistory, GridView, GridDelta, Cell, LineCounter, Color, CellAttributes
};

#[test]
fn test_grid_history_basic() {
    // Create initial grid
    let mut grid = Grid::new(80, 24);
    grid.timestamp = chrono::Utc::now();
    
    // Create history
    let mut history = GridHistory::new(grid.clone());
    
    // Add some test deltas
    for i in 0..10 {
        let mut delta = GridDelta::new();
        delta.add_cell_change(0, i as u16, Cell {
            char: ('A' as u8 + (i % 26) as u8) as char,
            fg_color: crate::server::terminal_state::Color::Default,
            bg_color: crate::server::terminal_state::Color::Default,
            attributes: crate::server::terminal_state::CellAttributes::default(),
        });
        history.add_delta(delta);
    }
    
    // Check that we can retrieve the current state
    let current = history.get_current().unwrap();
    assert_eq!(current.width, 80);
    assert_eq!(current.height, 24);
}

#[test]
fn test_grid_view_historical_mode() {
    // Create initial grid with some content
    let mut grid = Grid::new(80, 24);
    grid.timestamp = chrono::Utc::now();
    
    // Set line numbers for testing
    grid.start_line = LineCounter::from_u64(100);
    grid.end_line = LineCounter::from_u64(123);  // 24 lines
    
    // Fill grid with test content
    for row in 0..24 {
        for col in 0..10 {
            let ch = ((100 + row) % 10).to_string().chars().next().unwrap();
            grid.set_cell(row as u16, col as u16, Cell {
            char: ch,
            fg_color: crate::server::terminal_state::Color::Default,
            bg_color: crate::server::terminal_state::Color::Default,
            attributes: crate::server::terminal_state::CellAttributes::default(),
        });
        }
    }
    
    let history = Arc::new(Mutex::new(GridHistory::new(grid.clone())));
    let view = GridView::new(history.clone());
    
    // Test derive_from_line - request view from line 105
    let historical_grid = view.derive_from_line(105, Some(10)).unwrap();
    
    // Should start from line 105
    assert_eq!(historical_grid.start_line.to_u64().unwrap(), 105);
    assert_eq!(historical_grid.end_line.to_u64().unwrap(), 114);  // 10 lines
    assert_eq!(historical_grid.height, 10);
    
    // Verify content shifted correctly
    // Row 0 should contain content from original row 5 (line 105)
    let first_cell = historical_grid.get_cell(0, 0).unwrap();
    assert_eq!(first_cell.char, '5');  // (105 % 10) = 5
}

#[test]
fn test_grid_view_time_based() {
    // Create initial grid
    let mut grid = Grid::new(80, 24);
    let initial_time = chrono::Utc::now();
    grid.timestamp = initial_time;
    
    // Add initial content
    for col in 0..10 {
        grid.set_cell(0, col as u16, Cell {
            char: 'A',
            fg_color: crate::server::terminal_state::Color::Default,
            bg_color: crate::server::terminal_state::Color::Default,
            attributes: crate::server::terminal_state::CellAttributes::default(),
        });
    }
    
    let history_arc = Arc::new(Mutex::new(GridHistory::new(grid.clone())));
    
    // Add a delta after 1 second
    let later_time = initial_time + chrono::Duration::seconds(1);
    {
        let mut history = history_arc.lock().unwrap();
        let mut delta = GridDelta::new();
        delta.timestamp = later_time;
        delta.add_cell_change(0, 0, Cell {
            char: 'B',
            fg_color: crate::server::terminal_state::Color::Default,
            bg_color: crate::server::terminal_state::Color::Default,
            attributes: crate::server::terminal_state::CellAttributes::default(),
        });
        history.add_delta(delta);
    }
    
    // Create view and test time-based retrieval
    let view = GridView::new(history_arc.clone());
    
    // Get view at initial time - should show 'A'
    let initial_view = view.derive_at_time(initial_time, Some(24)).unwrap();
    assert_eq!(initial_view.get_cell(0, 0).unwrap().char, 'A');
    
    // Get view at later time - should show 'B'
    let later_view = view.derive_at_time(later_time, Some(24)).unwrap();
    assert_eq!(later_view.get_cell(0, 0).unwrap().char, 'B');
}

#[test]
fn test_scrollback_line_calculation() {
    // Create a grid with known line numbers
    let mut grid = Grid::new(80, 24);
    grid.start_line = LineCounter::from_u64(1000);
    grid.end_line = LineCounter::from_u64(1023);
    
    let history = Arc::new(Mutex::new(GridHistory::new(grid.clone())));
    let view = GridView::new(history.clone());
    
    // Request historical view from line 990 (before current view)
    // This simulates scrolling up to see older content
    let result = view.derive_from_line(990, Some(24));
    
    // Since line 990 is before our current start (1000), 
    // we should get the view starting from line 1000
    match result {
        Ok(historical_grid) => {
            assert_eq!(historical_grid.start_line.to_u64().unwrap(), 1000);
        }
        Err(_) => {
            // This is also acceptable - no history before line 1000
        }
    }
    
    // Request view from line 1010 (within current view)
    let mid_view = view.derive_from_line(1010, Some(10)).unwrap();
    assert_eq!(mid_view.start_line.to_u64().unwrap(), 1010);
    assert_eq!(mid_view.end_line.to_u64().unwrap(), 1019);
    assert_eq!(mid_view.height, 10);
}
use crate::server::terminal_state::*;
use std::sync::{Arc, Mutex};
use chrono::Utc;

#[test]
fn test_terminal_resize_preserves_content() {
    // Create an AlacrittyTerminal with initial content
    let mut terminal = AlacrittyTerminal::new(80, 24, None).unwrap();
    
    // Write some content
    let content = b"Line 1: Hello World\r\nLine 2: Testing resize\r\nLine 3: Content preservation";
    terminal.process_output(content).unwrap();
    
    // Get the grid before resize
    let grid_before = terminal.get_current_grid();
    
    // Verify initial content
    assert_eq!(grid_before.width, 80);
    assert_eq!(grid_before.height, 24);
    
    // Check first line content
    let mut line1 = String::new();
    for col in 0..19 {
        if let Some(cell) = grid_before.get_cell(0, col) {
            line1.push(cell.char);
        }
    }
    assert_eq!(line1.trim(), "Line 1: Hello World");
    
    // Resize the terminal
    terminal.resize(60, 20).unwrap();
    
    // Get the grid after resize
    let grid_after = terminal.get_current_grid();
    
    // Verify new dimensions
    assert_eq!(grid_after.width, 60);
    assert_eq!(grid_after.height, 20);
    
    // Verify content is preserved (may be rewrapped)
    // The exact position might change due to rewrapping, but content should exist
    let mut found_hello = false;
    let mut found_testing = false;
    for row in 0..grid_after.height {
        let mut line = String::new();
        for col in 0..grid_after.width {
            if let Some(cell) = grid_after.get_cell(row, col) {
                line.push(cell.char);
            }
        }
        if line.contains("Hello World") {
            found_hello = true;
        }
        if line.contains("Testing resize") {
            found_testing = true;
        }
    }
    assert!(found_hello, "Content 'Hello World' was lost during resize");
    assert!(found_testing, "Content 'Testing resize' was lost during resize");
}

#[test]
fn test_resize_creates_dimension_change_delta() {
    let mut terminal = AlacrittyTerminal::new(80, 24, None).unwrap();
    
    // Clear history
    {
        let history_arc = terminal.get_history(); let mut history = history_arc.lock().unwrap();
        history.clear_deltas();
    }
    
    // Resize the terminal
    terminal.resize(100, 30).unwrap();
    
    // Check that a delta with dimension change was created
    let history = terminal.get_history();
    let history_lock = history.lock().unwrap();
    
    // Should have at least one delta
    assert!(history_lock.has_deltas(), "No delta created for resize");
    
    // Find a delta with dimension change
    let has_dimension_change = history_lock.iter_deltas()
        .any(|delta| delta.dimension_change.is_some());
    
    assert!(has_dimension_change, "No dimension change recorded in deltas");
    
    // Check the dimension change details
    let dimension_delta = history_lock.iter_deltas()
        .find(|d| d.dimension_change.is_some())
        .unwrap();
    
    if let Some(ref dim_change) = dimension_delta.dimension_change {
        assert_eq!(dim_change.old_width, 80);
        assert_eq!(dim_change.old_height, 24);
        assert_eq!(dim_change.new_width, 100);
        assert_eq!(dim_change.new_height, 30);
    }
}

#[test]
fn test_resize_no_op_when_same_dimensions() {
    let mut terminal = AlacrittyTerminal::new(80, 24, None).unwrap();
    
    // Process some content
    terminal.process_output(b"Test content").unwrap();
    
    // Clear history to track new deltas
    {
        let history_arc = terminal.get_history(); let mut history = history_arc.lock().unwrap();
        history.clear_deltas();
    }
    
    // Resize to same dimensions
    terminal.resize(80, 24).unwrap();
    
    // Should not create any delta
    let history = terminal.get_history();
    let history_lock = history.lock().unwrap();
    assert!(!history_lock.has_deltas(), "Delta created for no-op resize");
}

#[test]
fn test_multiple_resizes_tracked_correctly() {
    let mut terminal = AlacrittyTerminal::new(80, 24, None).unwrap();
    
    // Clear history
    {
        let history_arc = terminal.get_history(); let mut history = history_arc.lock().unwrap();
        history.clear_deltas();
    }
    
    // Perform multiple resizes
    terminal.resize(100, 30).unwrap();
    terminal.resize(60, 20).unwrap();
    terminal.resize(120, 40).unwrap();
    
    // Check that all resizes are tracked
    let history = terminal.get_history();
    let history_lock = history.lock().unwrap();
    
    let dimension_changes: Vec<_> = history_lock.iter_deltas()
        .filter_map(|d| d.dimension_change.as_ref())
        .collect();
    
    assert_eq!(dimension_changes.len(), 3, "Not all resizes were tracked");
    
    // Verify the sequence
    assert_eq!(dimension_changes[0].new_width, 100);
    assert_eq!(dimension_changes[0].new_height, 30);
    
    assert_eq!(dimension_changes[1].old_width, 100);
    assert_eq!(dimension_changes[1].old_height, 30);
    assert_eq!(dimension_changes[1].new_width, 60);
    assert_eq!(dimension_changes[1].new_height, 20);
    
    assert_eq!(dimension_changes[2].old_width, 60);
    assert_eq!(dimension_changes[2].old_height, 20);
    assert_eq!(dimension_changes[2].new_width, 120);
    assert_eq!(dimension_changes[2].new_height, 40);
}

#[test]
fn test_resize_timing_in_output_sequence() {
    let mut terminal = AlacrittyTerminal::new(80, 24, None).unwrap();
    
    // Clear history
    {
        let history_arc = terminal.get_history(); let mut history = history_arc.lock().unwrap();
        history.clear_deltas();
    }
    
    // Process output, resize, then more output
    terminal.process_output(b"Before resize\r\n").unwrap();
    
    // Get delta count before resize
    let deltas_before = {
        let history_arc = terminal.get_history();
        let history = history_arc.lock().unwrap();
        history.delta_count()
    };
    
    // Resize
    terminal.resize(100, 30).unwrap();
    
    // Process more output
    terminal.process_output(b"After resize\r\n").unwrap();
    
    // Check that resize delta is in the right position
    let history = terminal.get_history();
    let history_lock = history.lock().unwrap();
    
    // Find the resize delta position
    let resize_position = history_lock.iter_deltas()
        .position(|d| d.dimension_change.is_some())
        .expect("Resize delta not found");
    
    // The resize should be after the "Before resize" deltas but before "After resize" deltas
    assert!(resize_position >= deltas_before, 
            "Resize delta appears before previous output");
    
    // Verify that there are deltas after the resize
    assert!(history_lock.delta_count() > resize_position + 1,
            "No output deltas after resize");
}

#[test]
fn test_grid_view_respects_resize() {
    let initial_grid = Grid::new(80, 24);
    let history = Arc::new(Mutex::new(GridHistory::new(initial_grid)));
    let mut view = GridView::new(Arc::clone(&history));
    
    // Add a resize delta
    let mut resize_delta = GridDelta {
        timestamp: Utc::now(),
        cell_changes: vec![],
        dimension_change: Some(DimensionChange {
            old_width: 80,
            old_height: 24,
            new_width: 100,
            new_height: 30,
        }),
        cursor_change: None,
        sequence: 1,
    };
    
    {
        let mut hist = history.lock().unwrap();
        hist.add_delta(resize_delta);
        // Update the current grid dimensions
        hist.current_grid_mut().width = 100;
        hist.current_grid_mut().height = 30;
    }
    
    // Get the grid view
    let grid = view.derive_realtime(None).unwrap();
    
    // Should reflect new dimensions
    assert_eq!(grid.width, 100);
    assert_eq!(grid.height, 30);
}

#[test]
fn test_resize_with_content_at_boundaries() {
    let mut terminal = AlacrittyTerminal::new(80, 24, None).unwrap();
    
    // Write content at the right edge
    let mut edge_content = vec![b' '; 79];
    edge_content.extend(b"X");  // Character at column 79 (last column)
    terminal.process_output(&edge_content).unwrap();
    
    // Write content at bottom edge
    for _ in 0..22 {
        terminal.process_output(b"\r\n").unwrap();
    }
    terminal.process_output(b"Bottom line").unwrap();
    
    // Resize smaller
    terminal.resize(40, 12).unwrap();
    
    let grid = terminal.get_current_grid();
    assert_eq!(grid.width, 40);
    assert_eq!(grid.height, 12);
    
    // Content should be handled appropriately (wrapped or truncated)
    // The exact behavior depends on the terminal implementation
    
    // Resize larger
    terminal.resize(120, 36).unwrap();
    
    let grid = terminal.get_current_grid();
    assert_eq!(grid.width, 120);
    assert_eq!(grid.height, 36);
}

#[test]
fn test_resize_race_condition_simulation() {
    // This test simulates what happens when resize occurs between output chunks
    let mut terminal = AlacrittyTerminal::new(80, 24, None).unwrap();
    
    // Simulate output being processed in chunks with resize in between
    terminal.process_output(b"First chunk of").unwrap();
    
    // Resize happens here (simulating SIGWINCH between reads)
    terminal.resize(100, 30).unwrap();
    
    // Continue processing output after resize
    terminal.process_output(b" output after resize").unwrap();
    
    // Verify both chunks are present and dimensions are correct
    let grid = terminal.get_current_grid();
    assert_eq!(grid.width, 100);
    assert_eq!(grid.height, 30);
    
    // Check that content from both chunks is present
    let mut content = String::new();
    for col in 0..grid.width.min(35) {
        if let Some(cell) = grid.get_cell(0, col) {
            if cell.char != ' ' {
                content.push(cell.char);
            }
        }
    }
    
    assert!(content.contains("First"), "First chunk lost");
    assert!(content.contains("output"), "Second chunk lost");
}
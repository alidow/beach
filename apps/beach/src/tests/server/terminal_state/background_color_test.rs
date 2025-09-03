use crate::server::terminal_state::{TerminalStateTracker, Color};

#[test]
fn test_background_color_preserved_on_clear() {
    let mut tracker = TerminalStateTracker::new(80, 24);
    
    // Set red background and clear screen
    let ansi_sequence = b"\x1b[41m\x1b[2J\x1b[H";
    tracker.process_output(ansi_sequence);
    
    // Force snapshot to get current state
    tracker.force_snapshot();
    
    // Get the current grid
    let history = tracker.get_history();
    let history_lock = history.lock().unwrap();
    let grid = history_lock.get_current().unwrap();
    
    // Check that the background is red (index 1 in the standard 8-color palette)
    // Red background is color index 1 (41 - 40 = 1)
    let expected_bg = Color::Indexed(1);
    
    // Check a few cells to ensure they have red background
    for row in 0..3 {
        for col in 0..5 {
            if let Some(cell) = grid.get_cell(row, col) {
                assert_eq!(
                    cell.bg_color, expected_bg,
                    "Cell at ({}, {}) should have red background", row, col
                );
                assert_eq!(
                    cell.char, ' ',
                    "Cell at ({}, {}) should be a space after clear", row, col
                );
            }
        }
    }
    
    // Cursor should be at home position (0, 0)
    assert_eq!(grid.cursor.row, 0, "Cursor should be at row 0 after \\x1b[H");
    assert_eq!(grid.cursor.col, 0, "Cursor should be at col 0 after \\x1b[H");
}

#[test]
fn test_erase_line_preserves_background() {
    let mut tracker = TerminalStateTracker::new(80, 24);
    
    // Type some text
    tracker.process_output(b"Hello World");
    
    // Set green background and erase line
    tracker.process_output(b"\x1b[42m\x1b[2K");
    
    tracker.force_snapshot();
    
    let history = tracker.get_history();
    let history_lock = history.lock().unwrap();
    let grid = history_lock.get_current().unwrap();
    
    // Green background is color index 2 (42 - 40 = 2)
    let expected_bg = Color::Indexed(2);
    
    // Check that the entire first line has green background
    for col in 0..10 {
        if let Some(cell) = grid.get_cell(0, col) {
            assert_eq!(
                cell.bg_color, expected_bg,
                "Cell at (0, {}) should have green background after erase line", col
            );
            assert_eq!(
                cell.char, ' ',
                "Cell at (0, {}) should be a space after erase line", col
            );
        }
    }
}

#[test]
fn test_partial_clear_preserves_background() {
    let mut tracker = TerminalStateTracker::new(80, 24);
    
    // Move cursor to middle of screen
    tracker.process_output(b"\x1b[12;40H");
    
    // Set blue background and clear from cursor to end
    tracker.process_output(b"\x1b[44m\x1b[0J");
    
    tracker.force_snapshot();
    
    let history = tracker.get_history();
    let history_lock = history.lock().unwrap();
    let grid = history_lock.get_current().unwrap();
    
    // Blue background is color index 4 (44 - 40 = 4)
    let expected_bg = Color::Indexed(4);
    
    // Check that cells from cursor position have blue background
    // Cursor is at row 11 (12-1), col 39 (40-1)
    for col in 39..45 {
        if let Some(cell) = grid.get_cell(11, col) {
            assert_eq!(
                cell.bg_color, expected_bg,
                "Cell at (11, {}) should have blue background", col
            );
        }
    }
    
    // Check a cell in the cleared area below
    if let Some(cell) = grid.get_cell(13, 0) {
        assert_eq!(
            cell.bg_color, expected_bg,
            "Cell at (13, 0) should have blue background"
        );
    }
}

#[test]
fn test_sgr_reset_clears_background() {
    let mut tracker = TerminalStateTracker::new(80, 24);
    
    // Set red background
    tracker.process_output(b"\x1b[41m");
    
    // Type some text with red background
    tracker.process_output(b"Red text");
    
    // Reset SGR (should clear background)
    tracker.process_output(b"\x1b[0m");
    
    // Type more text
    tracker.process_output(b" Normal");
    
    tracker.force_snapshot();
    
    let history = tracker.get_history();
    let history_lock = history.lock().unwrap();
    let grid = history_lock.get_current().unwrap();
    
    // Check that "Red text" has red background
    for col in 0..3 {
        if let Some(cell) = grid.get_cell(0, col) {
            assert_eq!(
                cell.bg_color, Color::Indexed(1),
                "First part should have red background at col {}", col
            );
        }
    }
    
    // Check that " Normal" has default background
    for col in 8..10 {
        if let Some(cell) = grid.get_cell(0, col) {
            assert_eq!(
                cell.bg_color, Color::Default,
                "Text after SGR reset should have default background at col {}", col
            );
        }
    }
}
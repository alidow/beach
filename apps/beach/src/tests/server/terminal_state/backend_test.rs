use crate::server::terminal_state::Color;
use super::test_utils::TestTerminal;

#[test]
fn test_background_color_preserved_on_clear_with_backend() {
    let mut terminal = TestTerminal::new(80, 24);
    
    // Set red background and clear screen
    let ansi_sequence = b"\x1b[41m\x1b[2J\x1b[H";
    terminal.process_output(ansi_sequence);
    
    // Force snapshot to get current state
    terminal.force_snapshot();
    
    // Get the current grid
    let history = terminal.get_history();
    let history_lock = history.lock().unwrap();
    let grid = history_lock.get_current().unwrap();
    
    // Check that the background is red (index 1 in the standard 8-color palette)
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
fn test_erase_line_preserves_background_with_backend() {
    let mut terminal = TestTerminal::new(80, 24);
    
    // Type some text
    terminal.process_output(b"Hello World");
    
    // Set green background and erase line
    terminal.process_output(b"\x1b[42m\x1b[2K");
    
    terminal.force_snapshot();
    
    let history = terminal.get_history();
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
fn test_basic_text_output_with_backend() {
    let mut terminal = TestTerminal::new(80, 24);
    
    // Output simple text
    let text = b"Hello, World!";
    terminal.process_output(text);
    
    terminal.force_snapshot();
    
    let grid = terminal.get_current_grid();
    
    // Check that text was written correctly
    let expected = "Hello, World!";
    for (i, ch) in expected.chars().enumerate() {
        if let Some(cell) = grid.get_cell(0, i as u16) {
            assert_eq!(
                cell.char, ch,
                "Character at position {} should be '{}'", i, ch
            );
        }
    }
}

#[test]
fn test_cursor_movement_with_backend() {
    let mut terminal = TestTerminal::new(80, 24);
    
    // Move cursor to position (5, 10) - ANSI uses 1-based indexing
    terminal.process_output(b"\x1b[6;11H");
    terminal.force_snapshot();
    
    let grid = terminal.get_current_grid();
    assert_eq!(grid.cursor.row, 5, "Cursor row should be 5");
    assert_eq!(grid.cursor.col, 10, "Cursor col should be 10");
}

#[test]
fn test_sgr_attributes_with_backend() {
    let mut terminal = TestTerminal::new(80, 24);
    
    // Set bold and red foreground
    terminal.process_output(b"\x1b[1;31mBold Red Text");
    terminal.force_snapshot();
    
    let grid = terminal.get_current_grid();
    
    // Check first character has bold and red attributes
    if let Some(cell) = grid.get_cell(0, 0) {
        assert_eq!(cell.char, 'B');
        assert_eq!(cell.fg_color, Color::Indexed(1)); // Red
        assert!(cell.attributes.bold, "Text should be bold");
    }
}

#[test]
fn test_newline_and_carriage_return_with_backend() {
    let mut terminal = TestTerminal::new(80, 24);
    
    // Test line feed
    terminal.process_output(b"Line 1\nLine 2");
    terminal.force_snapshot();
    
    let grid = terminal.get_current_grid();
    
    // Check Line 1 is on row 0
    if let Some(cell) = grid.get_cell(0, 0) {
        assert_eq!(cell.char, 'L');
    }
    
    // Check Line 2 is on row 1
    if let Some(cell) = grid.get_cell(1, 0) {
        assert_eq!(cell.char, 'L');
    }
    
    // Test carriage return
    let mut terminal = TestTerminal::new(80, 24);
    terminal.process_output(b"XXXXX\rYY");
    terminal.force_snapshot();
    
    let grid = terminal.get_current_grid();
    
    // Check that YY overwrote the first two X's
    if let Some(cell) = grid.get_cell(0, 0) {
        assert_eq!(cell.char, 'Y');
    }
    if let Some(cell) = grid.get_cell(0, 1) {
        assert_eq!(cell.char, 'Y');
    }
    if let Some(cell) = grid.get_cell(0, 2) {
        assert_eq!(cell.char, 'X');
    }
}
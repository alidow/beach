use super::test_utils::TestTerminal;
use crate::server::terminal_state::Color;

#[test_timeout::timeout]
fn test_debug_newline_alacritty() {
    let mut terminal = TestTerminal::new(80, 24);

    // Test line feed
    terminal.process_output(b"Line 1\nLine 2");
    terminal.force_snapshot();

    let grid = terminal.get_current_grid();

    // Debug print the grid
    println!("Grid after newline:");
    for row in 0..5 {
        print!("Row {}: ", row);
        for col in 0..10 {
            if let Some(cell) = grid.get_cell(row, col) {
                if cell.char == '\0' || cell.char == ' ' {
                    print!("_ ");
                } else {
                    print!("{} ", cell.char);
                }
            } else {
                print!("X ");
            }
        }
        println!();
    }

    // Check cursor position
    println!("Cursor: row={}, col={}", grid.cursor.row, grid.cursor.col);
}

#[cfg(feature = "alacritty-backend")]
use crate::server::terminal_state::AlacrittyTerminal;

#[cfg(feature = "alacritty-backend")]
#[test]
fn test_alacritty_line_feed_mode() {
    let mut terminal = AlacrittyTerminal::new(80, 24, None).unwrap();
    
    // Process "Line 1\nLine 2"
    terminal.process_output(b"Line 1\nLine 2").unwrap();
    terminal.force_snapshot();
    
    let grid = terminal.get_current_grid();
    
    // Debug print the grid
    println!("AlacrittyTerminal Grid after newline:");
    for row in 0..5 {
        print!("Row {}: ", row);
        for col in 0..15 {
            if let Some(cell) = grid.get_cell(row, col) {
                if cell.char == '\0' || cell.char == ' ' {
                    print!("_");
                } else {
                    print!("{}", cell.char);
                }
            } else {
                print!("X");
            }
        }
        println!();
    }
    
    println!("Cursor: row={}, col={}", grid.cursor.row, grid.cursor.col);
    
    // Check if Line 2 is on row 1
    if let Some(cell) = grid.get_cell(1, 0) {
        println!("Cell at (1,0): '{}'", cell.char);
    }
}
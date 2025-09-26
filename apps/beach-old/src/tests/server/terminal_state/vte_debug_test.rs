use crate::server::terminal_state::{TerminalStateTracker};

#[test_timeout::timeout]
fn test_debug_vte_newline() {
    let mut tracker = TerminalStateTracker::new(80, 24);
    
    // Test line feed
    tracker.process_output(b"Line 1\nLine 2");
    tracker.force_snapshot();
    
    let grid = {
        let history = tracker.get_history();
        let history_lock = history.lock().unwrap();
        history_lock.get_current().unwrap()
    };
    
    // Debug print the grid
    println!("VTE Grid after newline:");
    for row in 0..5 {
        print!("Row {}: ", row);
        for col in 0..15 {
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
#[cfg(feature = "alacritty-backend")]
#[test]
fn test_cr_clears_pending_wrap() {
    use crate::server::terminal_state::AlacrittyTerminal;

    // Test that CR clears pending wrap without advancing to next line
    let mut term = AlacrittyTerminal::new(10, 3, None, None).unwrap();

    // Output exactly 10 characters to fill the line
    term.process_output(b"0123456789").unwrap();

    let grid_at_eol = term.get_current_grid();
    println!("After filling line (10 chars on 10-wide terminal):");
    println!(
        "  Cursor: row={}, col={}",
        grid_at_eol.cursor.row, grid_at_eol.cursor.col
    );

    // The cursor should be at column 10 (off the edge) in pending wrap state
    // or at column 9 (last column) depending on implementation

    // Now send CR - this should clear pending wrap and move to column 0 of SAME row
    term.process_output(b"\r").unwrap();

    let grid_after_cr = term.get_current_grid();
    println!("After CR:");
    println!(
        "  Cursor: row={}, col={}",
        grid_after_cr.cursor.row, grid_after_cr.cursor.col
    );

    // The cursor should be at (0, 0) - same row, column 0
    assert_eq!(
        grid_after_cr.cursor.row, 0,
        "CR should not advance to next row from pending wrap"
    );
    assert_eq!(grid_after_cr.cursor.col, 0, "CR should move to column 0");

    // Now output more text - it should overwrite the existing line
    term.process_output(b"ABC").unwrap();

    let final_grid = term.get_current_grid();
    println!("After outputting 'ABC':");
    for row in 0..2 {
        let line = (0..10)
            .map(|col| {
                final_grid
                    .get_cell(row, col)
                    .map(|c| {
                        if c.char == ' ' || c.char == '\0' {
                            '_'
                        } else {
                            c.char
                        }
                    })
                    .unwrap_or('?')
            })
            .collect::<String>();
        println!("  Row {}: '{}'", row, line);
    }

    // Row 0 should be "ABC3456789" (ABC overwrote 012)
    assert_eq!(final_grid.get_cell(0, 0).unwrap().char, 'A');
    assert_eq!(final_grid.get_cell(0, 1).unwrap().char, 'B');
    assert_eq!(final_grid.get_cell(0, 2).unwrap().char, 'C');
    assert_eq!(final_grid.get_cell(0, 3).unwrap().char, '3');

    // Row 1 should be empty
    for col in 0..10 {
        let cell = final_grid.get_cell(1, col).unwrap();
        assert!(
            cell.char == ' ' || cell.char == '\0',
            "Row 1 col {} should be empty but has '{}'",
            col,
            cell.char
        );
    }
}

#[cfg(feature = "alacritty-backend")]
#[test_timeout::timeout]
fn test_clear_to_end_of_screen() {
    use crate::server::terminal_state::AlacrittyTerminal;

    let mut term = AlacrittyTerminal::new(10, 5, None, None).unwrap();

    // Fill the screen with content
    term.process_output(b"Line1\r\n").unwrap();
    term.process_output(b"Line2\r\n").unwrap();
    term.process_output(b"Line3\r\n").unwrap();
    term.process_output(b"Line4\r\n").unwrap();
    term.process_output(b"Line5").unwrap();

    let grid_before = term.get_current_grid();
    println!("Before clear:");
    for row in 0..5 {
        let line = (0..5)
            .map(|col| {
                grid_before
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

    // Move cursor to row 1, column 0
    term.process_output(b"\x1b[2;1H").unwrap();

    // Clear from cursor to end of screen
    term.process_output(b"\x1b[J").unwrap();

    let grid_after = term.get_current_grid();
    println!("\nAfter ESC[J from row 1:");
    for row in 0..5 {
        let line = (0..5)
            .map(|col| {
                grid_after
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

        // Check expectations
        if row == 0 {
            assert!(line.starts_with("Line1"), "Row 0 should still have Line1");
        } else {
            assert!(line == "_____", "Row {} should be cleared", row);
        }
    }
}

#[cfg(feature = "alacritty-backend")]
#[test_timeout::timeout]
fn test_clear_after_wrap() {
    use crate::server::terminal_state::AlacrittyTerminal;

    let mut term = AlacrittyTerminal::new(10, 3, None, None).unwrap();

    // Output text that wraps
    term.process_output(b"1234567890ABC").unwrap();

    let grid_after_wrap = term.get_current_grid();
    println!("After wrap:");
    println!(
        "  Cursor: row={}, col={}",
        grid_after_wrap.cursor.row, grid_after_wrap.cursor.col
    );
    for row in 0..3 {
        let line = (0..10)
            .map(|col| {
                grid_after_wrap
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

    // Now move cursor back to row 0 and clear
    term.process_output(b"\r").unwrap(); // CR to go to start of current line
    term.process_output(b"\x1b[A").unwrap(); // Move up one line
    term.process_output(b"\x1b[J").unwrap(); // Clear from cursor to end

    let grid_after_clear = term.get_current_grid();
    println!("\nAfter moving to row 0 and ESC[J:");
    println!(
        "  Cursor: row={}, col={}",
        grid_after_clear.cursor.row, grid_after_clear.cursor.col
    );
    for row in 0..3 {
        let line = (0..10)
            .map(|col| {
                grid_after_clear
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

    // All rows should be cleared
    for row in 0..3 {
        for col in 0..10 {
            let cell = grid_after_clear.get_cell(row, col).unwrap();
            assert!(
                cell.char == ' ' || cell.char == '\0',
                "Cell at ({}, {}) should be cleared, but has '{}'",
                row,
                col,
                cell.char
            );
        }
    }
}

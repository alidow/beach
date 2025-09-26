#[cfg(feature = "alacritty-backend")]
#[test_timeout::timeout]
fn test_prompt_eol_exact_sequence() {
    use crate::server::terminal_state::AlacrittyTerminal;

    // Recreate the EXACT sequence from zsh PROMPT_EOL_MARK
    let mut term = AlacrittyTerminal::new(154, 27, None, None).unwrap();

    // Initial state - empty terminal
    println!("\n=== Initial State ===");
    let initial_grid = term.get_current_grid();
    println!(
        "Cursor: row={}, col={}",
        initial_grid.cursor.row, initial_grid.cursor.col
    );

    // Step 1: Output command result without newline (like echo -n 'hello')
    term.process_output(b"hello").unwrap();
    let after_hello = term.get_current_grid();
    println!("\n=== After 'hello' (no newline) ===");
    println!(
        "Cursor: row={}, col={}",
        after_hello.cursor.row, after_hello.cursor.col
    );

    // Step 2: PROMPT_EOL_MARK sequence starts
    // 2a. Bold + Reverse video for %
    term.process_output(b"\x1b[1m\x1b[7m%\x1b[27m\x1b[1m\x1b[0m")
        .unwrap();
    let after_percent = term.get_current_grid();
    println!("\n=== After % with styling ===");
    println!(
        "Cursor: row={}, col={}",
        after_percent.cursor.row, after_percent.cursor.col
    );

    // 2b. Fill rest of line with spaces (154 - 6 = 148 spaces)
    let spaces = vec![b' '; 148];
    term.process_output(&spaces).unwrap();
    let after_spaces = term.get_current_grid();
    println!("\n=== After 148 spaces (should trigger wrap) ===");
    println!(
        "Cursor: row={}, col={}",
        after_spaces.cursor.row, after_spaces.cursor.col
    );
    println!("Expected: cursor at row=1, col=0 (wrapped to next line)");

    // 2c. CR to return to start of line
    term.process_output(b"\r").unwrap();
    let after_cr1 = term.get_current_grid();
    println!("\n=== After first CR ===");
    println!(
        "Cursor: row={}, col={}",
        after_cr1.cursor.row, after_cr1.cursor.col
    );

    // 2d. Space to overwrite any % if it's there
    term.process_output(b" ").unwrap();
    let after_space = term.get_current_grid();
    println!("\n=== After space (to clear %) ===");
    println!(
        "Cursor: row={}, col={}",
        after_space.cursor.row, after_space.cursor.col
    );

    // 2e. Another CR
    term.process_output(b"\r").unwrap();
    let after_cr2 = term.get_current_grid();
    println!("\n=== After second CR ===");
    println!(
        "Cursor: row={}, col={}",
        after_cr2.cursor.row, after_cr2.cursor.col
    );

    // 2f. Another CR (yes, zsh does three CRs)
    term.process_output(b"\r").unwrap();
    let after_cr3 = term.get_current_grid();
    println!("\n=== After third CR ===");
    println!(
        "Cursor: row={}, col={}",
        after_cr3.cursor.row, after_cr3.cursor.col
    );

    // 2g. Reset styles and clear to end of screen
    term.process_output(b"\x1b[0m\x1b[27m\x1b[24m\x1b[J")
        .unwrap();
    let after_clear = term.get_current_grid();
    println!("\n=== After style reset and ESC[J ===");
    println!(
        "Cursor: row={}, col={}",
        after_clear.cursor.row, after_clear.cursor.col
    );

    // Step 3: Output the prompt
    term.process_output(b"(base) prompt> ").unwrap();
    let final_grid = term.get_current_grid();
    println!("\n=== After prompt ===");
    println!(
        "Cursor: row={}, col={}",
        final_grid.cursor.row, final_grid.cursor.col
    );

    // Show the grid content
    println!("\n=== Final Grid Content ===");
    for row in 0..5 {
        let mut line = String::new();
        let mut has_content = false;
        for col in 0..40 {
            if let Some(cell) = final_grid.get_cell(row, col) {
                if cell.char != ' ' && cell.char != '\0' {
                    has_content = true;
                    line.push(cell.char);
                } else if has_content {
                    line.push(' ');
                } else {
                    line.push('_');
                }
            }
        }
        println!("Row {}: '{}'", row, line);
    }

    // Check what's on each row
    let row0_has_content = (0..final_grid.width).any(|col| {
        final_grid
            .get_cell(0, col)
            .map(|c| c.char != ' ' && c.char != '\0')
            .unwrap_or(false)
    });

    let row1_has_content = (0..final_grid.width).any(|col| {
        final_grid
            .get_cell(1, col)
            .map(|c| c.char != ' ' && c.char != '\0')
            .unwrap_or(false)
    });

    println!("\nRow 0 has content: {}", row0_has_content);
    println!("Row 1 has content: {}", row1_has_content);

    if !row0_has_content && row1_has_content {
        println!("✓ Content moved to row 1 (expected if wrap wasn't cleared)");
    } else if row0_has_content && !row1_has_content {
        println!("✓ Content stayed on row 0 (expected if wrap was properly handled)");
    } else if row0_has_content && row1_has_content {
        println!("⚠️ Content on both rows - unexpected");
    } else {
        println!("⚠️ No content on either row - unexpected");
    }
}

#[cfg(feature = "alacritty-backend")]
#[test_timeout::timeout]
fn test_wrap_with_one_extra_char() {
    use crate::server::terminal_state::AlacrittyTerminal;

    // Test what happens when we output exactly one character past line end
    let mut term = AlacrittyTerminal::new(10, 3, None, None).unwrap();

    // Output 10 chars (fills line) + 1 more
    term.process_output(b"0123456789X").unwrap();

    let grid = term.get_current_grid();
    println!("\nAfter outputting 11 chars on 10-wide terminal:");
    println!("Cursor: row={}, col={}", grid.cursor.row, grid.cursor.col);

    // X should be on row 1
    assert_eq!(
        grid.get_cell(1, 0).unwrap().char,
        'X',
        "X should wrap to next line"
    );
    assert_eq!(grid.cursor.row, 1, "Cursor should be on row 1");
    assert_eq!(grid.cursor.col, 1, "Cursor should be at col 1 (after X)");
}

#[cfg(feature = "alacritty-backend")]
#[test_timeout::timeout]
fn test_wrap_then_cr_behavior() {
    use crate::server::terminal_state::AlacrittyTerminal;

    // Test CR behavior after actual wrap (not pending wrap)
    let mut term = AlacrittyTerminal::new(10, 3, None, None).unwrap();

    // Output 11 chars to cause wrap
    term.process_output(b"0123456789X").unwrap();

    let before_cr = term.get_current_grid();
    println!("\nBefore CR:");
    println!(
        "Cursor: row={}, col={}",
        before_cr.cursor.row, before_cr.cursor.col
    );

    // Send CR
    term.process_output(b"\r").unwrap();

    let after_cr = term.get_current_grid();
    println!("After CR:");
    println!(
        "Cursor: row={}, col={}",
        after_cr.cursor.row, after_cr.cursor.col
    );

    // CR should move to start of CURRENT line (row 1)
    assert_eq!(after_cr.cursor.row, 1, "CR should stay on current row");
    assert_eq!(after_cr.cursor.col, 0, "CR should move to column 0");
}

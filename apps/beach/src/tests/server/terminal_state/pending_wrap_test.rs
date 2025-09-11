#[cfg(feature = "alacritty-backend")]
#[test]
fn test_pending_wrap_vs_immediate_wrap() {
    use crate::server::terminal_state::AlacrittyTerminal;
    
    // Test the specific PROMPT_EOL_MARK behavior on a 154-column terminal
    let mut term = AlacrittyTerminal::new(154, 27, None, None).unwrap();
    
    // First, output a line that doesn't end with newline to trigger PROMPT_EOL_MARK
    // Output "hello" which is 5 characters, putting cursor at column 5
    term.process_output(b"hello").unwrap();
    
    let grid_after_hello = term.get_current_grid();
    println!("After 'hello': cursor at row={}, col={}", 
             grid_after_hello.cursor.row, grid_after_hello.cursor.col);
    
    // Now the shell will output PROMPT_EOL_MARK:
    // 1. Reverse video % at current position (column 5), making it column 6
    let mut eol_sequence = Vec::new();
    eol_sequence.extend_from_slice(b"\x1b[1m\x1b[7m%\x1b[27m\x1b[1m\x1b[0m");
    
    // 2. We need to reach column 153 (last column, 0-indexed)
    // We're at column 6 after the %, so we need 153 - 6 = 147 more spaces
    // NO WAIT - let me check the actual zsh behavior
    // The PROMPT_EOL_MARK prints % + enough spaces to fill the rest of the line
    // So from column 6, we need 154 - 6 = 148 spaces to reach the end
    for _ in 0..148 {
        eol_sequence.push(b' ');
    }
    
    // 3. CR, space, CR (to clear the % if no wrap occurred)
    eol_sequence.extend_from_slice(b"\r \r");
    // 4. Clear to end of screen - this should remove phantom lines
    eol_sequence.extend_from_slice(b"\r\x1b[0m\x1b[27m\x1b[24m\x1b[J");
    
    term.process_output(&eol_sequence).unwrap();
    
    let grid = term.get_current_grid();
    
    // Debug output
    println!("\n=== Pending Wrap Test ===");
    println!("Terminal dimensions: {}x{}", grid.width, grid.height);
    println!("Cursor position: row={}, col={}", grid.cursor.row, grid.cursor.col);
    
    // Let's see what happens if we continue with a prompt
    term.process_output(b"prompt> ").unwrap();
    let grid_with_prompt = term.get_current_grid();
    
    println!("After adding prompt:");
    println!("Cursor position: row={}, col={}", grid_with_prompt.cursor.row, grid_with_prompt.cursor.col);
    
    // Check first 3 rows
    for row in 0..3 {
        print!("Row {}: '", row);
        let mut line = String::new();
        let mut has_content = false;
        
        for col in 0..std::cmp::min(grid.width, 40) {
            if let Some(cell) = grid.get_cell(row, col) {
                if cell.char != ' ' && cell.char != '\0' {
                    has_content = true;
                    line.push(cell.char);
                } else if has_content {
                    line.push('_'); // Show spaces after content
                } else {
                    line.push('_'); // Leading spaces
                }
            }
        }
        println!("{}...' (first 40 chars)", line);
    }
    
    // The key question: Is there a blank line after row 0?
    let row1_has_content = (0..grid_with_prompt.width).any(|col| {
        grid_with_prompt.get_cell(1, col)
            .map(|c| c.char != ' ' && c.char != '\0')
            .unwrap_or(false)
    });
    
    // Actually, let me check if the issue is that when we output the prompt,
    // it goes on row 0 (which is correct), but the grid rendering sees row 1 as blank
    // This could be a display issue rather than a terminal emulation issue
    
    // Let's check row 0 content after prompt is added
    let row0_content = (0..40).map(|col| {
        grid_with_prompt.get_cell(0, col)
            .map(|c| if c.char == ' ' || c.char == '\0' { '_' } else { c.char })
            .unwrap_or('?')
    }).collect::<String>();
    
    println!("Row 0 after prompt: '{}'", row0_content);
    
    if !row1_has_content && grid.cursor.row == 0 {
        // This is actually the expected behavior!
        // The cursor stayed on row 0, and row 1 should be blank
        // The issue is that in the actual terminal display, we don't SEE row 1
        // because it was never scrolled into view
        println!("\n✓ EXPECTED: Cursor on row 0, row 1 is blank");
        println!("The blank line exists in the grid but shouldn't be visible");
        println!("This is correct terminal state - the issue is in how we display it");
    } else if row1_has_content {
        println!("\n⚠️ Unexpected: Row 1 has content when it should be blank");
        panic!("Row 1 has content - this is unexpected");
    } else {
        println!("\n⚠️ Cursor moved to row 1 - immediate wrap occurred");
        panic!("Immediate wrap detected - cursor moved to row 1");
    }
}

#[cfg(feature = "alacritty-backend")]
#[test]
fn test_wrap_at_exact_boundary() {
    use crate::server::terminal_state::AlacrittyTerminal;
    
    // Test wrap behavior when text exactly fills the line
    let mut term = AlacrittyTerminal::new(10, 5, None, None).unwrap();
    
    // Output exactly 10 characters (fills the line)
    term.process_output(b"0123456789").unwrap();
    
    let grid = term.get_current_grid();
    println!("\n=== Exact Boundary Test (10 chars on 10-wide terminal) ===");
    println!("Cursor after 10 chars: row={}, col={}", grid.cursor.row, grid.cursor.col);
    
    // Now output CR
    term.process_output(b"\r").unwrap();
    let grid_after_cr = term.get_current_grid();
    println!("Cursor after CR: row={}, col={}", grid_after_cr.cursor.row, grid_after_cr.cursor.col);
    
    // Check that we're still on row 0
    assert_eq!(grid_after_cr.cursor.row, 0, "CR should keep us on same row in pending wrap state");
    assert_eq!(grid_after_cr.cursor.col, 0, "CR should move cursor to column 0");
    
    // Now add more text
    term.process_output(b"ABC").unwrap();
    let final_grid = term.get_current_grid();
    
    // The ABC should overwrite the start of the line
    assert_eq!(final_grid.get_cell(0, 0).unwrap().char, 'A');
    assert_eq!(final_grid.get_cell(0, 1).unwrap().char, 'B');
    assert_eq!(final_grid.get_cell(0, 2).unwrap().char, 'C');
    
    println!("✓ Text correctly overwrites after CR in pending wrap state");
}
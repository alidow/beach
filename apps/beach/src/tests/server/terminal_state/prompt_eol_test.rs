#[cfg(feature = "alacritty-backend")]
#[test]
fn test_prompt_eol_mark_sequence() {
    use crate::server::terminal_state::AlacrittyTerminal;
    
    // Create terminal with actual dimensions from the issue
    let mut term = AlacrittyTerminal::new(154, 27, None, None).unwrap();
    
    // Simulate: echo 'hi' followed by PROMPT_EOL_MARK with 153 spaces
    let mut sequence = Vec::new();
    sequence.extend_from_slice(b"hi\r\n");
    sequence.extend_from_slice(b"\x1b[1m\x1b[7m%\x1b[27m\x1b[1m\x1b[0m");
    for _ in 0..153 {
        sequence.push(b' ');
    }
    sequence.extend_from_slice(b"\r \r\r\x1b[0m\x1b[27m\x1b[24m\x1b[Jprompt> ");
    
    term.process_output(&sequence).unwrap();
    
    let grid = term.get_current_grid();
    
    // Debug print first few lines of grid
    println!("Grid after PROMPT_EOL_MARK (154x27):");
    for row in 0..5 {
        print!("Row {}: '", row);
        for col in 0..30 {
            if let Some(cell) = grid.get_cell(row, col) {
                if cell.char == '\0' || cell.char == ' ' {
                    print!("_");
                } else {
                    print!("{}", cell.char);
                }
            }
        }
        println!("...'");
    }
    
    // Check expectations
    // Line 0 should have "hi"
    assert_eq!(grid.get_cell(0, 0).unwrap().char, 'h');
    assert_eq!(grid.get_cell(0, 1).unwrap().char, 'i');
    
    // Force output by panicking with the grid state
    let line1_char = grid.get_cell(1, 0).unwrap().char;
    if line1_char != 'p' {
        panic!("Expected prompt on line 1, but got '{}'. Line 2 first char: '{}'", 
               line1_char, 
               grid.get_cell(2, 0).map(|c| c.char).unwrap_or('?'))
    }
}
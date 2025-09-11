use crate::server::terminal_state::*;

#[test]
fn test_unix_newline_behavior() {
    // Test what actually happens with Unix-style newlines (just \n)
    let mut terminal = AlacrittyTerminal::new(80, 24, None, None).unwrap();
    
    // Simulate what a real shell outputs - just '\n' for Unix
    terminal.process_output(b"echo 'First line'").unwrap();
    terminal.process_output(b"\n").unwrap(); // Unix newline - should now work with our fix
    terminal.process_output(b"First line").unwrap();
    terminal.process_output(b"\n").unwrap();
    
    terminal.process_output(b"echo 'Second line'").unwrap();
    terminal.process_output(b"\n").unwrap();
    terminal.process_output(b"Second line").unwrap(); 
    terminal.process_output(b"\n").unwrap();
    
    let grid = terminal.get_current_grid();
    
    // Debug print
    println!("\nActual grid output (first 8 rows):");
    for row in 0..8 {
        print!("Row {:2}: |", row);
        for col in 0..40 {
            let cell = grid.get_cell(row, col).unwrap();
            print!("{}", cell.char);
        }
        println!("|");
    }
    
    // Check that lines appear on correct rows WITHOUT extra blank lines
    // Row 0: echo 'First line'
    // Row 1: First line
    // Row 2: echo 'Second line'
    // Row 3: Second line
    
    // Check "echo 'First line'" is at row 0
    let text = "echo 'First line'";
    for (i, ch) in text.chars().enumerate() {
        let cell = grid.get_cell(0, i as u16).unwrap();
        assert_eq!(cell.char, ch, "Row 0 col {} mismatch", i);
    }
    
    // Check "First line" is at row 1 (NOT row 2!)
    let text = "First line";
    for (i, ch) in text.chars().enumerate() {
        let cell = grid.get_cell(1, i as u16).unwrap();
        assert_eq!(cell.char, ch, "Row 1 col {} mismatch", i);
    }
    
    // Check "echo 'Second line'" is at row 2 (NOT row 3 or 4!)
    let text = "echo 'Second line'";
    for (i, ch) in text.chars().enumerate() {
        let cell = grid.get_cell(2, i as u16).unwrap();
        assert_eq!(cell.char, ch, "Row 2 col {} mismatch", i);
    }
    
    // Check "Second line" is at row 3 (NOT row 4 or 5!)
    let text = "Second line";
    for (i, ch) in text.chars().enumerate() {
        let cell = grid.get_cell(3, i as u16).unwrap();
        assert_eq!(cell.char, ch, "Row 3 col {} mismatch", i);
    }
}

#[test]
fn test_newline_vs_crlf_behavior() {
    // Compare \n vs \r\n behavior
    
    // Test 1: Just \n (Unix style)
    let mut terminal1 = AlacrittyTerminal::new(80, 24, None, None).unwrap();
    terminal1.process_output(b"Line1\nLine2\nLine3").unwrap();
    let grid1 = terminal1.get_current_grid();
    
    // Test 2: \r\n (Windows style)  
    let mut terminal2 = AlacrittyTerminal::new(80, 24, None, None).unwrap();
    terminal2.process_output(b"Line1\r\nLine2\r\nLine3").unwrap();
    let grid2 = terminal2.get_current_grid();
    
    // Both should produce the same result - 3 lines on rows 0, 1, 2
    for row in 0..3 {
        for col in 0..10 {
            let cell1 = grid1.get_cell(row, col).unwrap();
            let cell2 = grid2.get_cell(row, col).unwrap();
            assert_eq!(cell1.char, cell2.char, 
                "Mismatch at row {} col {}: '{}' vs '{}'", row, col, cell1.char, cell2.char);
        }
    }
}
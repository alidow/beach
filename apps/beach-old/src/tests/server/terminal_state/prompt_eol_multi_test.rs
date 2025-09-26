#[cfg(feature = "alacritty-backend")]
#[test_timeout::timeout]
fn test_multiple_commands_with_prompt_eol() {
    use crate::server::terminal_state::AlacrittyTerminal;

    let mut term = AlacrittyTerminal::new(154, 27, None, None).unwrap();

    // First command: echo 'hello'
    let mut seq1 = Vec::new();
    seq1.extend_from_slice(b"(base) prompt> echo 'hello'\r\n");
    seq1.extend_from_slice(b"hello\r\n");
    // PROMPT_EOL_MARK after output
    seq1.extend_from_slice(b"\x1b[1m\x1b[7m%\x1b[27m\x1b[1m\x1b[0m");
    for _ in 0..153 {
        seq1.push(b' ');
    }
    seq1.extend_from_slice(b"\r \r\r\x1b[0m\x1b[27m\x1b[24m\x1b[J");
    seq1.extend_from_slice(b"(base) prompt> ");

    term.process_output(&seq1).unwrap();

    // Second command: echo 'world'
    let mut seq2 = Vec::new();
    seq2.extend_from_slice(b"echo 'world'\r\n");
    seq2.extend_from_slice(b"world\r\n");
    // PROMPT_EOL_MARK after output
    seq2.extend_from_slice(b"\x1b[1m\x1b[7m%\x1b[27m\x1b[1m\x1b[0m");
    for _ in 0..153 {
        seq2.push(b' ');
    }
    seq2.extend_from_slice(b"\r \r\r\x1b[0m\x1b[27m\x1b[24m\x1b[J");
    seq2.extend_from_slice(b"(base) prompt> ");

    term.process_output(&seq2).unwrap();

    let grid = term.get_current_grid();

    // Print the grid
    println!("Grid after two commands:");
    for row in 0..10 {
        print!("Row {:2}: '", row);
        for col in 0..40 {
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

    // Check that there are no extra blank lines
    // We should see:
    // Row 0: (base) prompt> echo 'hello'
    // Row 1: hello
    // Row 2: (base) prompt> echo 'world'
    // Row 3: world
    // Row 4: (base) prompt>

    // Check for the pattern
    assert!(
        grid.get_cell(0, 0).unwrap().char == '(' || grid.get_cell(0, 0).unwrap().char == ' ',
        "Row 0 should start with prompt or be blank from scrolling"
    );

    // Count blank lines
    let mut blank_lines = 0;
    for row in 0..10 {
        let mut is_blank = true;
        for col in 0..40 {
            if let Some(cell) = grid.get_cell(row, col) {
                if cell.char != ' ' && cell.char != '\0' {
                    is_blank = false;
                    break;
                }
            }
        }
        if is_blank {
            blank_lines += 1;
            println!("Row {} is blank", row);
        }
    }

    println!("Found {} blank lines in first 10 rows", blank_lines);
    assert!(
        blank_lines <= 5,
        "Too many blank lines - likely PROMPT_EOL_MARK issue"
    );
}

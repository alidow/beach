use crate::server::terminal_state::*;
use chrono::Utc;
use std::sync::{Arc, Mutex};

#[test]
fn test_basic_terminal_state() {
    // Create a 80x24 terminal
    let mut tracker = TerminalStateTracker::new(80, 24);

    // Test 1: Initial state - all cells should be default
    {
        let history = tracker.get_history();
        let history_lock = history.lock().unwrap();
        let initial_grid = history_lock.get_current().unwrap();

        assert_eq!(initial_grid.width, 80);
        assert_eq!(initial_grid.height, 24);

        // Check that all cells are spaces with default colors
        for row in 0..24 {
            for col in 0..80 {
                let cell = initial_grid.get_cell(row, col).unwrap();
                assert_eq!(cell.char, ' ');
                assert_eq!(cell.fg_color, Color::Default);
                assert_eq!(cell.bg_color, Color::Default);
            }
        }
    }

    // Test 2: Process simple text
    let text = b"Hello World";
    tracker.process_output(text);

    {
        let history = tracker.get_history();
        let history_lock = history.lock().unwrap();
        let grid = history_lock.get_current().unwrap();

        // Check that "Hello World" appears at position (0,0)
        let expected = "Hello World";
        for (i, ch) in expected.chars().enumerate() {
            let cell = grid.get_cell(0, i as u16).unwrap();
            assert_eq!(cell.char, ch, "Character mismatch at column {}", i);
        }
    }

    // Test 3: Process newline
    let text_with_newline = b"\nSecond line";
    tracker.process_output(text_with_newline);

    {
        let history = tracker.get_history();
        let history_lock = history.lock().unwrap();
        let grid = history_lock.get_current().unwrap();

        // "Second line" should be on row 1
        let expected = "Second line";
        for (i, ch) in expected.chars().enumerate() {
            let cell = grid.get_cell(1, i as u16).unwrap();
            assert_eq!(cell.char, ch, "Character mismatch at row 1, column {}", i);
        }
    }

    // Test 4: Process ANSI color sequence
    let color_sequence = b"\x1b[31mRed Text\x1b[0m";
    tracker.process_output(color_sequence);

    {
        let history = tracker.get_history();
        let history_lock = history.lock().unwrap();
        let grid = history_lock.get_current().unwrap();

        // Find "Red Text" and verify it has red color
        // It should be on row 1 after "Second line"
        let start_col = 11; // After "Second line"
        let expected = "Red Text";
        for (i, ch) in expected.chars().enumerate() {
            let cell = grid.get_cell(1, (start_col + i) as u16).unwrap();
            assert_eq!(cell.char, ch);
            // Color index 1 is red in 8-color palette
            assert_eq!(cell.fg_color, Color::Indexed(1));
        }
    }
}

#[test]
fn test_grid_delta() {
    let mut grid1 = Grid::new(10, 10);
    let mut grid2 = grid1.clone();

    // Modify grid2
    grid2.set_cell(
        5,
        5,
        Cell {
            char: 'X',
            fg_color: Color::Rgb(255, 0, 0),
            bg_color: Color::Default,
            attributes: CellAttributes::default(),
        },
    );

    // Create delta
    let delta = GridDelta::diff(&grid1, &grid2);

    // Should have exactly one cell change
    assert_eq!(delta.cell_changes.len(), 1);
    assert_eq!(delta.cell_changes[0].row, 5);
    assert_eq!(delta.cell_changes[0].col, 5);
    assert_eq!(delta.cell_changes[0].old_cell.char, ' ');
    assert_eq!(delta.cell_changes[0].new_cell.char, 'X');

    // Apply delta to original grid
    delta.apply(&mut grid1).unwrap();

    // Now grid1 should match grid2
    let cell = grid1.get_cell(5, 5).unwrap();
    assert_eq!(cell.char, 'X');
    assert_eq!(cell.fg_color, Color::Rgb(255, 0, 0));
}

#[test]
fn test_grid_history() {
    let initial_grid = Grid::new(10, 10);
    let mut history = GridHistory::new(initial_grid);

    // Add some deltas
    for i in 0..150 {
        let mut delta = GridDelta {
            timestamp: Utc::now(),
            cell_changes: vec![CellChange {
                row: 0,
                col: i % 10,
                old_cell: Cell::default(),
                new_cell: Cell {
                    char: (b'A' + (i as u8 % 26)) as char,
                    fg_color: Color::Default,
                    bg_color: Color::Default,
                    attributes: CellAttributes::default(),
                },
            }],
            dimension_change: None,
            cursor_change: None,
            sequence: 0,
        };

        history.add_delta(delta);

        // Sleep briefly to respect snapshot timing
        if i == 100 {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    // Should have created at least one snapshot (at delta 100)
    let current = history.get_current().unwrap();

    // Verify the last change was applied
    let last_col = 149 % 10;
    let expected_char = (b'A' + (149u8 % 26)) as char;
    let cell = current.get_cell(0, last_col).unwrap();
    assert_eq!(cell.char, expected_char);
}

#[test]
fn test_grid_view_height_truncation() {
    let history = Arc::new(Mutex::new(GridHistory::new(Grid::new(80, 24))));
    let view = GridView::new(Arc::clone(&history));

    // Get current view with limited height
    let truncated = view.derive_realtime(Some(12)).unwrap();

    assert_eq!(truncated.width, 80); // Width should remain original
    assert_eq!(truncated.height, 12); // Height should be limited
}

#[test]
fn test_cursor_movement() {
    let mut tracker = TerminalStateTracker::new(80, 24);

    // Move cursor using ANSI sequence (ESC[5;10H moves to row 5, col 10)
    let cursor_move = b"\x1b[5;10H";
    tracker.process_output(cursor_move);

    // Write text at new position
    let text = b"Positioned";
    tracker.process_output(text);

    {
        let history = tracker.get_history();
        let history_lock = history.lock().unwrap();
        let grid = history_lock.get_current().unwrap();

        // Check text appears at row 4, col 9 (0-indexed)
        let expected = "Positioned";
        for (i, ch) in expected.chars().enumerate() {
            let cell = grid.get_cell(4, (9 + i) as u16).unwrap();
            assert_eq!(
                cell.char,
                ch,
                "Character mismatch at row 4, column {}",
                9 + i
            );
        }
    }
}

#[test]
fn test_clear_screen() {
    let mut tracker = TerminalStateTracker::new(80, 24);

    // Write some text
    tracker.process_output(b"Some text here");
    tracker.process_output(b"\nMore text");

    // Clear screen (ESC[2J)
    tracker.process_output(b"\x1b[2J");

    {
        let history = tracker.get_history();
        let history_lock = history.lock().unwrap();
        let grid = history_lock.get_current().unwrap();

        // All cells should be spaces again
        for row in 0..24 {
            for col in 0..80 {
                let cell = grid.get_cell(row, col).unwrap();
                assert_eq!(
                    cell.char, ' ',
                    "Cell at ({}, {}) should be space after clear",
                    row, col
                );
            }
        }
    }
}

#[test]
fn test_emoji_handling() {
    let mut tracker = TerminalStateTracker::new(80, 24);

    // Process emoji text - use just the base emoji without variation selector
    let emoji_text = "🏖 Beach".as_bytes();
    tracker.process_output(emoji_text);

    {
        let history = tracker.get_history();
        let history_lock = history.lock().unwrap();
        let grid = history_lock.get_current().unwrap();

        // Check that emoji is stored correctly
        let cell0 = grid.get_cell(0, 0).unwrap();
        assert_eq!(cell0.char, '🏖');

        // The emoji might be treated as single width in some cases
        // Let's check where the space actually appears
        let mut space_pos = None;
        for col in 1..10 {
            if grid.get_cell(0, col).unwrap().char == ' ' {
                space_pos = Some(col);
                break;
            }
        }

        // Space should be at position 1 (if emoji is single width) or 2 (if double width)
        assert!(space_pos.is_some(), "Space not found after emoji");
        let space_col = space_pos.unwrap();

        // Then check "Beach" starts right after the space
        let expected = "Beach";
        for (i, ch) in expected.chars().enumerate() {
            let cell = grid.get_cell(0, (space_col + 1 + i as u16)).unwrap();
            assert_eq!(
                cell.char,
                ch,
                "Character mismatch at position {}",
                space_col + 1 + i as u16
            );
        }
    }
}

#[test]
fn test_memory_limit() {
    let initial_grid = Grid::new(80, 24);
    let mut history = GridHistory::new(initial_grid);

    // Generate a lot of data to trigger memory limit
    for i in 0..10000 {
        let delta = GridDelta {
            timestamp: Utc::now(),
            cell_changes: vec![CellChange {
                row: i % 24,
                col: i % 80,
                old_cell: Cell::default(),
                new_cell: Cell {
                    char: 'X',
                    fg_color: Color::Default,
                    bg_color: Color::Default,
                    attributes: CellAttributes::default(),
                },
            }],
            dimension_change: None,
            cursor_change: None,
            sequence: 0,
        };

        history.add_delta(delta);
    }

    // Should still be able to get current state
    let current = history.get_current();
    assert!(
        current.is_ok(),
        "Should be able to get current state after memory limit"
    );
}

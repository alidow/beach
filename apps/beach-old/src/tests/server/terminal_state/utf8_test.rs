use crate::server::terminal_state::{Cell, CellAttributes, Color, Grid, GridHistory, GridView};
use std::sync::{Arc, Mutex};

#[test_timeout::timeout]
fn test_grid_view_utf8_height_truncation() {
    // Create a grid with multi-byte UTF-8 characters
    let mut grid = Grid::new(100, 10);

    // Add a line with box-drawing characters
    let box_chars = "─────────────────────────────────────────────────────────────────────────────────────────────────";

    for (col, ch) in box_chars.chars().enumerate() {
        if col >= 100 {
            break;
        }
        let cell = Cell {
            char: ch,
            fg_color: Color::Default,
            bg_color: Color::Default,
            attributes: CellAttributes::default(),
        };
        grid.set_cell(0, col as u16, cell);
    }

    // Create a grid history and view
    let history = Arc::new(Mutex::new(GridHistory::new(grid.clone())));
    let view = GridView::new(history);

    // Test height truncation preserves width
    let truncated_grid = view.derive_realtime(Some(5)).unwrap();

    // Verify width remains unchanged
    assert_eq!(truncated_grid.width, 100);
    assert_eq!(truncated_grid.height, 5);

    // Check that the first row has box characters
    let first_char = truncated_grid.get_cell(0, 0);
    assert!(first_char.is_some());
    if let Some(cell) = first_char {
        assert_eq!(cell.char, '─');
    }
}

#[test_timeout::timeout]
fn test_grid_view_mixed_utf8_with_height() {
    // Test with mixed ASCII and multi-byte UTF-8 characters
    let mut grid = Grid::new(80, 10);

    // Add lines with mixed characters including emojis and box-drawing
    let lines = [
        "Hello 世界 🌍 ─────── Test 测试",
        "Second line with UTF-8: 你好",
        "Third line: こんにちは",
        "Box drawing: ┌─────┐",
        "More content: 🎨🎭🎪",
    ];

    for (row_idx, line) in lines.iter().enumerate() {
        for (col, ch) in line.chars().enumerate() {
            if col >= 80 {
                break;
            }
            let cell = Cell {
                char: ch,
                fg_color: Color::Default,
                bg_color: Color::Default,
                attributes: CellAttributes::default(),
            };
            grid.set_cell(row_idx as u16, col as u16, cell);
        }
    }

    let history = Arc::new(Mutex::new(GridHistory::new(grid.clone())));
    let view = GridView::new(history);

    // Test various height truncations
    for height in [1, 3, 5, 8, 10].iter() {
        let truncated_grid = view.derive_realtime(Some(*height)).unwrap();
        assert_eq!(truncated_grid.width, 80); // Width should remain unchanged
        assert_eq!(truncated_grid.height, *height.min(&10));

        // Verify UTF-8 characters are preserved
        if *height >= 1 {
            let cell = truncated_grid.get_cell(0, 6).unwrap(); // '世' character position
            assert_eq!(cell.char, '世');
        }
    }
}

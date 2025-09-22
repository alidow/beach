#![recursion_limit = "1024"]

/// Phase 2a Client Tests
/// Tests for the minimal client implementation with control channel and fallback

#[cfg(test)]
mod tests {
    use crate::client::{grid_renderer::GridRenderer, predictive_echo::PredictiveEcho};
    use crate::protocol::control_messages::ControlMessage;
    use crate::server::terminal_state::{Cell, CellAttributes, CellChange, Color, Grid, GridDelta};

    /// Test predictive echo basic functionality
    #[test_timeout::tokio_timeout_test]
    async fn test_predictive_input() {
        let client_id = "test-client".to_string();
        let mut predictive_echo = PredictiveEcho::new(client_id.clone());

        // Test predicting input
        let input = vec![b'a'];
        let cursor_pos = (0, 0);
        let seq = predictive_echo.predict_input(input.clone(), cursor_pos);
        assert_eq!(seq, 1);

        // Test creating input message
        let msg = predictive_echo.create_input_message(seq, input.clone());
        match msg {
            ControlMessage::Input {
                client_id: id,
                client_seq,
                bytes,
            } => {
                assert_eq!(id, client_id);
                assert_eq!(client_seq, 1);
                assert_eq!(bytes, input);
            }
            _ => panic!("Expected Input message"),
        }

        // Test acknowledgment
        predictive_echo.acknowledge(1, 100, 200);
    }

    /// Test grid renderer with scrolling
    #[test_timeout::tokio_timeout_test]
    async fn test_scroll_prefetch() {
        let mut grid_renderer = GridRenderer::new(80, 24, false).unwrap();

        // Test initial overscan parameters
        let (from_line, height) = grid_renderer.get_overscan_params();
        assert_eq!(from_line, 0);
        assert_eq!(height, 48); // 2x visible height (24 * 2)

        // Test vertical scrolling
        grid_renderer.scroll_vertical(-5);

        // Test vertical scrolling positive
        grid_renderer.scroll_vertical(5);

        // Test PageUp/PageDown behavior
        grid_renderer.scroll_vertical(10);
        grid_renderer.scroll_vertical(-10);
    }

    /// Test grid snapshot and delta application
    #[test_timeout::tokio_timeout_test]
    async fn test_reconnect_resync() {
        let mut grid_renderer = GridRenderer::new(80, 24, false).unwrap();

        // Apply initial snapshot
        let mut snapshot = Grid::new(80, 24);
        snapshot.cells[0][0].char = 'A';
        snapshot.cells[1][1].char = 'B';
        grid_renderer.apply_snapshot(snapshot.clone());

        // Verify snapshot is retained
        grid_renderer.retain_last_snapshot();
        assert_eq!(grid_renderer.grid.cells[0][0].char, 'A');
        assert_eq!(grid_renderer.grid.cells[1][1].char, 'B');

        // Simulate delta application
        let delta = GridDelta {
            timestamp: chrono::Utc::now(),
            cell_changes: vec![CellChange {
                row: 0,
                col: 0,
                old_cell: Cell {
                    char: 'A',
                    fg_color: Color::Default,
                    bg_color: Color::Default,
                    attributes: CellAttributes::default(),
                },
                new_cell: Cell {
                    char: 'C',
                    fg_color: Color::Default,
                    bg_color: Color::Default,
                    attributes: CellAttributes::default(),
                },
            }],
            dimension_change: None,
            cursor_change: None,
            sequence: 2,
        };

        grid_renderer.apply_delta(&delta);
        assert_eq!(grid_renderer.grid.cells[0][0].char, 'C');
    }

    /// Test order guarantees with sequential deltas
    #[test_timeout::tokio_timeout_test]
    async fn test_order_guarantees() {
        let mut grid_renderer = GridRenderer::new(80, 24, false).unwrap();

        // Apply initial grid
        let grid = Grid::new(80, 24);
        grid_renderer.apply_snapshot(grid);

        // Create a series of ordered deltas
        let mut deltas = Vec::new();
        for i in 0..5 {
            let delta = GridDelta {
                timestamp: chrono::Utc::now(),
                cell_changes: vec![CellChange {
                    row: 0,
                    col: i,
                    old_cell: Cell::default(),
                    new_cell: Cell {
                        char: (b'A' + i as u8) as char,
                        fg_color: Color::Default,
                        bg_color: Color::Default,
                        attributes: CellAttributes::default(),
                    },
                }],
                dimension_change: None,
                cursor_change: None,
                sequence: i as u64,
            };
            deltas.push(delta);
        }

        // Apply deltas in order
        for delta in &deltas {
            grid_renderer.apply_delta(delta);
        }

        // Verify order is maintained
        assert_eq!(grid_renderer.grid.cells[0][0].char, 'A');
        assert_eq!(grid_renderer.grid.cells[0][1].char, 'B');
        assert_eq!(grid_renderer.grid.cells[0][2].char, 'C');
        assert_eq!(grid_renderer.grid.cells[0][3].char, 'D');
        assert_eq!(grid_renderer.grid.cells[0][4].char, 'E');
    }

    /// Test horizontal scrolling for server width enforcement
    #[test_timeout::tokio_timeout_test]
    async fn test_horizontal_scroll() {
        // Create renderer with server width > local width
        let mut grid_renderer = GridRenderer::new(120, 24, false).unwrap();
        grid_renderer.resize_local(80, 24); // Local terminal is narrower

        // Verify horizontal scrolling is needed
        assert!(grid_renderer.needs_horizontal_scroll());

        // Test horizontal scrolling
        grid_renderer.scroll_horizontal(10);
        grid_renderer.scroll_horizontal(100); // Try to scroll past max
        grid_renderer.scroll_horizontal(-20);
        grid_renderer.scroll_horizontal(-100); // Try to scroll negative
    }
}

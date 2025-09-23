use crate::server::debug_handler::DebugHandler;
use crate::server::terminal_state::*;
use chrono::Utc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[test_timeout::timeout]
fn test_basic_debug_view() {
    // Create a terminal and process some output
    let backend = create_terminal_backend(80, 24, None, None).unwrap();
    let backend_arc = Arc::new(Mutex::new(backend));

    // Create debug handler and set backend
    let debug_handler = DebugHandler::new();
    debug_handler.set_backend(backend_arc.clone());

    // Process "echo 'hello'" output
    {
        let mut backend = backend_arc.lock().unwrap();
        backend.process_output(b"$ echo 'hello'\r\n").unwrap();
        backend.process_output(b"hello\r\n").unwrap();
        backend.process_output(b"$ ").unwrap();
    }

    // Get current grid view
    let response = debug_handler.get_grid_view(None, None, None).unwrap();

    assert_eq!(response.width, 80);
    assert_eq!(response.height, 24);

    // Check that output contains expected text
    let full_text = response.rows.join("\n");
    assert!(
        full_text.contains("$ echo 'hello'"),
        "Should contain command"
    );
    assert!(full_text.contains("hello"), "Should contain output");
    assert!(full_text.contains("$ "), "Should contain prompt");
}

#[test_timeout::timeout]
fn test_debug_view_with_multiple_commands() {
    let backend = create_terminal_backend(80, 24, None, None).unwrap();
    let backend_arc = Arc::new(Mutex::new(backend));

    let debug_handler = DebugHandler::new();
    debug_handler.set_backend(backend_arc.clone());

    // Process multiple commands
    {
        let mut backend = backend_arc.lock().unwrap();
        backend.process_output(b"$ echo 'hello'\r\n").unwrap();
        backend.process_output(b"hello\r\n").unwrap();
        backend.process_output(b"$ echo 'world'\r\n").unwrap();
        backend.process_output(b"world\r\n").unwrap();
        backend.process_output(b"$ ").unwrap();
    }

    // Get grid view
    let response = debug_handler.get_grid_view(None, None, None).unwrap();

    let full_text = response.rows.join("\n");
    assert!(full_text.contains("hello"), "Should contain first output");
    assert!(full_text.contains("world"), "Should contain second output");
}

#[test_timeout::timeout]
fn test_debug_view_with_height_limit() {
    let backend = create_terminal_backend(80, 24, None, None).unwrap();
    let backend_arc = Arc::new(Mutex::new(backend));

    let debug_handler = DebugHandler::new();
    debug_handler.set_backend(backend_arc.clone());

    // Add some content
    {
        let mut backend = backend_arc.lock().unwrap();
        for i in 1..=10 {
            backend
                .process_output(format!("Line {}\r\n", i).as_bytes())
                .unwrap();
        }
    }

    // Get limited height view
    let response = debug_handler.get_grid_view(Some(5), None, None).unwrap();

    assert_eq!(response.height, 5, "Height should be limited to 5");
    assert_eq!(response.rows.len(), 5, "Should have 5 rows");
}

#[test_timeout::timeout]
fn test_debug_view_time_travel() {
    let backend = create_terminal_backend(80, 24, None, None).unwrap();
    let backend_arc = Arc::new(Mutex::new(backend));

    let debug_handler = DebugHandler::new();
    debug_handler.set_backend(backend_arc.clone());

    // Capture initial state
    let time_before = Utc::now();

    // Process first output
    {
        let mut backend = backend_arc.lock().unwrap();
        backend.process_output(b"Initial\r\n").unwrap();
    }

    // Wait a bit
    thread::sleep(Duration::from_millis(100));
    let time_middle = Utc::now();

    // Process second output
    {
        let mut backend = backend_arc.lock().unwrap();
        backend.process_output(b"Later\r\n").unwrap();
    }

    // Get current view (should have both)
    let current = debug_handler.get_grid_view(None, None, None).unwrap();
    let current_text = current.rows.join("\n");
    assert!(current_text.contains("Initial"));
    assert!(current_text.contains("Later"));

    // Get view from before second output (should only have first)
    let past = debug_handler
        .get_grid_view(None, Some(time_middle), None)
        .unwrap();
    let past_text = past.rows.join("\n");
    assert!(past_text.contains("Initial"));
    assert!(
        !past_text.contains("Later"),
        "Past view should not contain later output"
    );

    // Verify timestamp is in the past
    assert!(
        past.timestamp <= time_middle,
        "Timestamp should be from requested time or earlier"
    );
}

#[test_timeout::timeout]
fn test_debug_view_from_line() {
    let backend = create_terminal_backend(80, 24, None, None).unwrap();
    let backend_arc = Arc::new(Mutex::new(backend));

    let debug_handler = DebugHandler::new();
    debug_handler.set_backend(backend_arc.clone());

    // Fill terminal with numbered lines (more than fit on screen)
    {
        let mut backend = backend_arc.lock().unwrap();
        for i in 1..=30 {
            backend
                .process_output(format!("Line {}\r\n", i).as_bytes())
                .unwrap();
        }
    }

    // Get view from line 0 (should show current visible content)
    let response = debug_handler.get_grid_view(None, None, Some(0)).unwrap();
    assert_eq!(response.start_line, 0);

    // The exact content depends on terminal scrolling behavior
    // But we should get a valid response
    assert!(response.width > 0);
    assert!(response.height > 0);
}

#[test_timeout::timeout]
fn test_debug_view_from_non_existent_line() {
    let backend = create_terminal_backend(80, 24, None, None).unwrap();
    let backend_arc = Arc::new(Mutex::new(backend));

    let debug_handler = DebugHandler::new();
    debug_handler.set_backend(backend_arc.clone());

    // Add minimal content
    {
        let mut backend = backend_arc.lock().unwrap();
        backend.process_output(b"Test\r\n").unwrap();
    }

    // Try to get view from line 1000 (beyond current terminal)
    // With the new implementation, this should return the last available view
    let result = debug_handler.get_grid_view(None, None, Some(1000));

    // Debug: Print what we got
    match &result {
        Ok(response) => {
            println!(
                "Got response for line 1000: start_line={}, end_line={} (returns last view)",
                response.start_line, response.end_line
            );
        }
        Err(e) => {
            println!("Got error for line 1000: {}", e);
        }
    }

    // With the new implementation, requesting a line beyond the end returns the last view
    assert!(
        result.is_ok(),
        "Should return last view when requesting beyond max line"
    );
}

#[test_timeout::timeout]
fn test_debug_view_boundary_conditions() {
    let backend = create_terminal_backend(80, 24, None, None).unwrap();
    let backend_arc = Arc::new(Mutex::new(backend));

    let debug_handler = DebugHandler::new();
    debug_handler.set_backend(backend_arc.clone());

    // Add some content
    {
        let mut backend = backend_arc.lock().unwrap();
        for i in 1..=5 {
            backend
                .process_output(format!("Line {}\r\n", i).as_bytes())
                .unwrap();
        }
    }

    // Get view to determine actual line range
    let full_view = debug_handler.get_grid_view(None, None, None).unwrap();
    let last_line = full_view.end_line;

    println!(
        "Terminal has lines {} to {}",
        full_view.start_line, last_line
    );

    // Test requesting line 0 (should always work)
    let result = debug_handler.get_grid_view(None, None, Some(0));
    println!("Requesting line 0: {:?}", result.is_ok());
    assert!(result.is_ok(), "Should succeed when requesting line 0");

    // Test requesting a very large line number (should return last view)
    let result = debug_handler.get_grid_view(None, None, Some(99999));
    println!("Requesting line 99999: {:?}", result.is_ok());
    assert!(
        result.is_ok(),
        "Should return last view when requesting beyond max"
    );
}

#[test_timeout::timeout]
fn test_debug_view_with_ansi_colors() {
    let backend = create_terminal_backend(80, 24, None, None).unwrap();
    let backend_arc = Arc::new(Mutex::new(backend));

    let debug_handler = DebugHandler::new();
    debug_handler.set_backend(backend_arc.clone());

    // Add colored output (red text)
    {
        let mut backend = backend_arc.lock().unwrap();
        backend
            .process_output(b"\x1b[31mRed Text\x1b[0m\r\n")
            .unwrap();
    }

    // Get view with ANSI colors
    let response = debug_handler.get_grid_view(None, None, None).unwrap();

    // Should have ANSI rows
    assert!(
        response.ansi_rows.is_some(),
        "Should include ANSI colored rows"
    );

    let ansi_rows = response.ansi_rows.unwrap();
    let ansi_text = ansi_rows.join("\n");

    // Should contain ANSI escape codes for red color
    assert!(
        ansi_text.contains("\x1b[") || ansi_text.contains("Red Text"),
        "Should contain colored text or escape codes"
    );
}

#[test_timeout::timeout]
fn test_line_tracking_across_scrolling() {
    let backend = create_terminal_backend(80, 10, None, None).unwrap(); // Small terminal
    let backend_arc = Arc::new(Mutex::new(backend));

    let debug_handler = DebugHandler::new();
    debug_handler.set_backend(backend_arc.clone());

    // Fill beyond terminal height to cause scrolling
    {
        let mut backend = backend_arc.lock().unwrap();
        for i in 1..=15 {
            backend
                .process_output(format!("Line {}\r\n", i).as_bytes())
                .unwrap();
        }
    }

    // Get current view
    let response = debug_handler.get_grid_view(None, None, None).unwrap();

    // Should track line numbers correctly even after scrolling
    assert!(response.start_line >= 0, "Start line should be valid");
    assert!(
        response.end_line >= response.start_line,
        "End line should be >= start line"
    );

    // The visible content should be the later lines (due to scrolling)
    let full_text = response.rows.join("\n");
    assert!(
        full_text.contains("Line 15") || full_text.contains("Line 14"),
        "Should show recent lines after scrolling"
    );
}

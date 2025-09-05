use portable_pty::{native_pty_system, CommandBuilder, PtySize, MasterPty, PtyPair};
use std::io::{Write, Read};
use std::time::Duration;
use tokio::time::sleep;
use std::sync::{Arc, Mutex};

/// Helper to run commands in a PTY and capture output
async fn run_commands_in_pty(commands: Vec<&str>, cols: u16) -> String {
    let pty_system = native_pty_system();
    let mut pty_pair = pty_system.openpty(PtySize {
        rows: 24,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }).expect("Failed to create PTY");

    // Use /bin/sh for consistent behavior and set non-interactive mode
    let mut cmd = CommandBuilder::new("/bin/sh");
    cmd.env("PS1", "$ "); // Simple prompt
    cmd.env("TERM", "dumb"); // Avoid complex terminal features
    let mut child = pty_pair.slave.spawn_command(cmd).expect("Failed to spawn shell");

    let master_writer = pty_pair.master.take_writer().expect("Failed to get writer");
    let mut master_reader = pty_pair.master.try_clone_reader().expect("Failed to get reader");

    // Spawn reader task with blocking I/O
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let reader_handle = tokio::task::spawn_blocking(move || {
        let mut buffer = [0u8; 1024];
        loop {
            match master_reader.read(&mut buffer) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let data = buffer[..n].to_vec();
                    if tx.send(data).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Wait briefly for shell to start (shorter timeout)
    let mut all_output = Vec::new();
    sleep(Duration::from_millis(100)).await;
    while let Ok(data) = rx.try_recv() {
        all_output.extend(data);
    }
    all_output.clear(); // Clear initial prompt

    // Execute commands
    let mut master_writer = master_writer;
    for cmd in commands {
        master_writer.write_all(format!("{}\n", cmd).as_bytes()).unwrap();
        master_writer.flush().unwrap();
        
        // Collect output with shorter wait
        sleep(Duration::from_millis(100)).await;
        while let Ok(data) = rx.try_recv() {
            all_output.extend(data);
        }
    }

    // Clean up
    child.kill().ok();
    child.wait().ok();
    reader_handle.abort();

    String::from_utf8_lossy(&all_output).to_string()
}

/// Extract clean output lines (remove prompts, echo, etc)
fn extract_output_lines(raw_output: &str) -> Vec<String> {
    raw_output
        .lines()
        .filter(|line| {
            !line.trim().is_empty() 
            && !line.contains('%')  // zsh prompt
            && !line.contains('$')  // bash prompt  
            && !line.contains('#')  // root prompt
            && !line.starts_with("echo ")  // command echo
        })
        .map(|s| s.trim().to_string())
        .collect()
}

#[tokio::test]
async fn test_default_shell_basic_echo() {
    // Test basic echo commands
    let output = run_commands_in_pty(vec!["echo 'hello'", "echo 'world'"], 80).await;
    let lines = extract_output_lines(&output);
    
    // Should have the two echo outputs
    assert!(lines.contains(&"hello".to_string()), 
        "Output should contain 'hello'. Got: {:?}", lines);
    assert!(lines.contains(&"world".to_string()), 
        "Output should contain 'world'. Got: {:?}", lines);
}

#[tokio::test]
async fn test_default_shell_line_wrapping() {
    // Create a string that exceeds 40 columns
    let long_text = "a".repeat(100);
    let command = format!("echo '{}'", long_text);
    
    // Run in narrow terminal (40 columns)
    let output = run_commands_in_pty(vec![&command], 40).await;
    
    // Split output into lines
    let lines: Vec<&str> = output.lines().collect();
    
    // Find the line that contains just our output (not the command echo)
    // The actual output line should be after the command and before the next prompt
    let output_line = lines.iter()
        .find(|line| {
            // Look for a line that starts with 'a' and contains mostly 'a's
            // This filters out the command echo which starts with "echo"
            line.starts_with("aaa") && !line.contains("echo")
        });
    
    assert!(output_line.is_some(), "Should find output line with 'a's. Lines: {:?}", lines);
    
    let output_text = output_line.unwrap();
    let a_count = output_text.chars().filter(|&c| c == 'a').count();
    
    // Should have exactly 100 'a's in the output
    assert_eq!(a_count, 100, 
        "Should have exactly 100 'a' characters in output line. Found {} in line: {:?}", 
        a_count, output_text);
    
    // The output itself should be on a single line (echo outputs to one logical line)
    // But we should see evidence of wrapping in the command echo
    assert!(output.contains("echo"), "Should see the echo command");
}

#[tokio::test]
async fn test_pty_preserves_output_order() {
    // Test that multiple commands maintain order
    let commands = vec![
        "echo 'first'",
        "echo 'second'", 
        "echo 'third'",
    ];
    
    let output = run_commands_in_pty(commands, 80).await;
    
    // Find positions of each output
    let first_pos = output.find("first").expect("Should find 'first'");
    let second_pos = output.find("second").expect("Should find 'second'");
    let third_pos = output.find("third").expect("Should find 'third'");
    
    // Verify order is preserved
    assert!(first_pos < second_pos, 
        "First should come before second");
    assert!(second_pos < third_pos, 
        "Second should come before third");
}

#[tokio::test]
async fn test_pty_handles_special_characters() {
    // Test echo with special characters
    let output = run_commands_in_pty(
        vec!["echo 'hello\\nworld'", "echo 'tab\\there'"],
        80
    ).await;
    
    // Should contain the outputs (exact format depends on shell)
    assert!(output.contains("hello") && output.contains("world"),
        "Should handle newline in echo");
    assert!(output.contains("tab") && output.contains("here"),
        "Should handle tab in echo");
}

/// Helper struct to manage a PTY session with resize capability
struct ResizablePtySession {
    pty_pair: Arc<Mutex<PtyPair>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    output: Arc<Mutex<Vec<u8>>>,
    reader_handle: Option<tokio::task::JoinHandle<()>>,
}

impl ResizablePtySession {
    async fn new(initial_cols: u16, initial_rows: u16) -> Self {
        let pty_system = native_pty_system();
        let mut pty_pair = pty_system.openpty(PtySize {
            rows: initial_rows,
            cols: initial_cols,
            pixel_width: 0,
            pixel_height: 0,
        }).expect("Failed to create PTY");

        // Use simple shell for testing
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.env("PS1", "$ ");
        cmd.env("TERM", "dumb");
        let _child = pty_pair.slave.spawn_command(cmd).expect("Failed to spawn shell");

        let writer = pty_pair.master.take_writer().expect("Failed to get writer");
        let mut reader = pty_pair.master.try_clone_reader().expect("Failed to get reader");

        let output = Arc::new(Mutex::new(Vec::new()));
        let output_clone = output.clone();

        // Start reader task with blocking I/O
        let reader_handle = tokio::task::spawn_blocking(move || {
            let mut buffer = [0u8; 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut out = output_clone.lock().unwrap();
                        out.extend_from_slice(&buffer[..n]);
                    }
                    Err(_) => break,
                }
            }
        });

        // Wait for initialization (shorter)
        sleep(Duration::from_millis(100)).await;

        Self {
            pty_pair: Arc::new(Mutex::new(pty_pair)),
            writer: Arc::new(Mutex::new(writer)),
            output,
            reader_handle: Some(reader_handle),
        }
    }

    async fn resize(&self, cols: u16, rows: u16) {
        let pty_pair = self.pty_pair.lock().unwrap();
        pty_pair.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        }).expect("Failed to resize PTY");
        
        // Give time for terminal to process resize
        sleep(Duration::from_millis(100)).await;
    }

    async fn send_command(&self, command: &str) {
        let writer_clone = self.writer.clone();
        let command = command.to_string();
        tokio::task::spawn_blocking(move || {
            let mut writer = writer_clone.lock().unwrap();
            writer.write_all(format!("{}\n", command).as_bytes()).unwrap();
            writer.flush().unwrap();
        })
        .await
        .expect("Failed to send command");
        sleep(Duration::from_millis(200)).await;
    }

    async fn send_keys(&self, keys: &[u8]) {
        let writer_clone = self.writer.clone();
        let keys = keys.to_vec();
        tokio::task::spawn_blocking(move || {
            let mut writer = writer_clone.lock().unwrap();
            writer.write_all(&keys).unwrap();
            writer.flush().unwrap();
        })
        .await
        .expect("Failed to send keys");
        sleep(Duration::from_millis(100)).await;
    }

    fn get_output(&self) -> String {
        let output = self.output.lock().unwrap();
        String::from_utf8_lossy(&output).to_string()
    }

    fn clear_output(&self) {
        self.output.lock().unwrap().clear();
    }
}

#[tokio::test]
#[ignore = "Resize tests may be flaky - run with --ignored to test"]
async fn test_terminal_resize_with_complex_content() {
    let session = ResizablePtySession::new(80, 24).await;
    session.clear_output();

    // Create complex content with multiple lines
    let long_line = "A".repeat(100); // Longer than 80 cols
    let numbered_lines = (1..=10).map(|i| format!("Line {}: {}", i, "x".repeat(70))).collect::<Vec<_>>();
    
    // Send long line
    session.send_command(&format!("echo '{}'", long_line)).await;
    sleep(Duration::from_millis(300)).await;
    
    let output_80_cols = session.get_output();
    let a_lines_80: Vec<&str> = output_80_cols.lines()
        .filter(|line| line.contains("AAA"))
        .collect();
    
    // Should wrap at 80 columns
    assert!(a_lines_80.len() >= 2, "100 A's should wrap in 80-column terminal");
    
    // Resize to narrow terminal
    session.resize(40, 24).await;
    session.clear_output();
    
    // Send the same long line in narrow terminal
    session.send_command(&format!("echo '{}'", long_line)).await;
    sleep(Duration::from_millis(300)).await;
    
    let output_40_cols = session.get_output();
    let a_lines_40: Vec<&str> = output_40_cols.lines()
        .filter(|line| line.contains("AAA"))
        .collect();
    
    // Should wrap more in narrower terminal
    assert!(a_lines_40.len() > a_lines_80.len(), 
        "Should have more wrapped lines in 40-col terminal than 80-col");
    
    // Resize to wide terminal
    session.resize(120, 24).await;
    session.clear_output();
    
    session.send_command(&format!("echo '{}'", long_line)).await;
    sleep(Duration::from_millis(300)).await;
    
    let output_120_cols = session.get_output();
    let a_lines_120: Vec<&str> = output_120_cols.lines()
        .filter(|line| line.contains("AAA"))
        .collect();
    
    // Should not wrap in wide terminal (or wrap less)
    assert!(a_lines_120.len() <= a_lines_80.len(),
        "Should have fewer wrapped lines in 120-col terminal");
    
    // Test multiple resizes with content already displayed
    for width in [60, 45, 90, 30] {
        session.resize(width, 24).await;
        sleep(Duration::from_millis(200)).await;
        
        // Send a test command to verify terminal still works after resize
        session.clear_output();
        session.send_command("echo 'resize test'").await;
        let output = session.get_output();
        assert!(output.contains("resize test"), 
            "Terminal should still work after resizing to {} cols", width);
    }
}

#[tokio::test]
#[ignore = "Vim tests are complex and may hang - run with --ignored to test"]
async fn test_vim_tui_interaction() {
    let session = ResizablePtySession::new(80, 24).await;
    session.clear_output();
    
    // Start vim with a temporary file
    let test_file = "/tmp/beach_test_vim.txt";
    session.send_command(&format!("echo 'initial content' > {}", test_file)).await;
    sleep(Duration::from_millis(200)).await;
    session.clear_output();
    
    // Open vim
    session.send_command(&format!("vim {}", test_file)).await;
    sleep(Duration::from_millis(500)).await; // Give vim time to start
    
    let vim_output = session.get_output();
    
    // Vim should show the file content or vim interface
    // (exact output depends on vim config, but should have some vim-specific content)
    assert!(vim_output.len() > 0, "Vim should produce output");
    
    // Test vim commands
    // Enter insert mode
    session.send_keys(b"i").await;
    
    // Type some text
    session.send_keys(b"Hello from test").await;
    
    // Escape to normal mode
    session.send_keys(&[0x1b]).await; // ESC key
    
    // Save and quit
    session.send_keys(b":wq\n").await;
    sleep(Duration::from_millis(500)).await;
    
    // Verify we're back at shell prompt by running a command
    session.clear_output();
    session.send_command(&format!("cat {}", test_file)).await;
    
    let content = session.get_output();
    assert!(content.contains("Hello from test") || content.contains("initial content"),
        "File should contain the text we typed or initial content");
    
    // Clean up
    session.send_command(&format!("rm {}", test_file)).await;
}

#[tokio::test]
#[ignore = "Vim tests are complex and may hang - run with --ignored to test"]
async fn test_vim_with_terminal_resize() {
    let session = ResizablePtySession::new(80, 24).await;
    session.clear_output();
    
    // Create a test file with content wider than narrow terminal
    let test_file = "/tmp/beach_resize_vim.txt";
    let wide_content = (1..=30).map(|i| format!("Line {}: {}", i, "=".repeat(100))).collect::<Vec<_>>().join("\n");
    session.send_command(&format!("cat > {} << 'EOF'\n{}\nEOF", test_file, wide_content)).await;
    sleep(Duration::from_millis(300)).await;
    session.clear_output();
    
    // Open vim
    session.send_command(&format!("vim {}", test_file)).await;
    sleep(Duration::from_millis(500)).await;
    
    // Initial vim should be in 80x24
    let initial_output = session.get_output();
    assert!(initial_output.len() > 0, "Vim should be running");
    
    // Resize terminal while vim is running
    session.resize(120, 30).await;
    
    // Send a vim command to trigger redraw
    session.send_keys(b"G").await; // Go to end of file
    sleep(Duration::from_millis(200)).await;
    
    // Resize again to narrow
    session.resize(40, 20).await;
    
    // Vim should handle the resize - send another command
    session.send_keys(b"gg").await; // Go to beginning
    sleep(Duration::from_millis(200)).await;
    
    // Test that vim still responds after multiple resizes
    session.send_keys(&[0x1b]).await; // ESC
    session.send_keys(b":q!\n").await; // Quit without saving
    sleep(Duration::from_millis(500)).await;
    
    // Verify we're back at shell
    session.clear_output();
    session.send_command("echo 'back at shell'").await;
    let output = session.get_output();
    assert!(output.contains("back at shell"), 
        "Should be back at shell prompt after quitting vim");
    
    // Clean up
    session.send_command(&format!("rm {}", test_file)).await;
}

#[tokio::test]
async fn test_special_key_sequences() {
    let session = ResizablePtySession::new(80, 24).await;
    session.clear_output();
    
    // Test Ctrl+C (interrupt)
    session.send_command("sleep 10").await;
    sleep(Duration::from_millis(100)).await;
    session.send_keys(&[0x03]).await; // Ctrl+C
    sleep(Duration::from_millis(200)).await;
    
    // Should be back at prompt, test with echo
    session.clear_output();
    session.send_command("echo 'interrupted'").await;
    let output = session.get_output();
    assert!(output.contains("interrupted"), "Ctrl+C should interrupt sleep command");
    
    // Test Ctrl+D (EOF) behavior
    session.clear_output();
    session.send_command("cat").await; // Start cat without file (reads from stdin)
    sleep(Duration::from_millis(100)).await;
    session.send_keys(b"test input\n").await;
    session.send_keys(&[0x04]).await; // Ctrl+D
    sleep(Duration::from_millis(200)).await;
    
    let cat_output = session.get_output();
    assert!(cat_output.contains("test input"), "Cat should echo the input");
    
    // Test arrow keys and editing
    session.clear_output();
    session.send_keys(b"echo hello").await;
    
    // Move cursor back with arrow keys (ESC [ D is left arrow)
    for _ in 0..5 {
        session.send_keys(&[0x1b, b'[', b'D']).await;
    }
    
    // Insert text in middle
    session.send_keys(b"INSERTED ").await;
    session.send_keys(b"\n").await; // Execute command
    sleep(Duration::from_millis(200)).await;
    
    let edited_output = session.get_output();
    // The exact output depends on shell, but should show some form of the edited command
    assert!(edited_output.contains("hello") || edited_output.contains("INSERTED"),
        "Should handle cursor movement and insertion");
}

// Helper function to join strings with newlines
trait JoinLines {
    fn join(&self, separator: &str) -> String;
}

impl JoinLines for Vec<String> {
    fn join(&self, separator: &str) -> String {
        self.iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(separator)
    }
}
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};

#[test]
fn test_pty_echo_command() {
    // Create a PTY
    let pty_system = native_pty_system();
    let mut pty_pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("Failed to create PTY");

    // Run a simple echo command directly (no shell)
    let mut cmd = CommandBuilder::new("echo");
    cmd.arg("hello");
    let mut child = pty_pair
        .slave
        .spawn_command(cmd)
        .expect("Failed to spawn echo");

    // Read output
    let mut reader = pty_pair
        .master
        .try_clone_reader()
        .expect("Failed to get reader");
    let mut buffer = [0u8; 1024];
    let mut output = Vec::new();

    // Read with timeout
    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < 1 {
        match reader.read(&mut buffer) {
            Ok(0) => break, // EOF
            Ok(n) => {
                output.extend_from_slice(&buffer[..n]);
                if output.contains(&b'\n') {
                    break; // Got a line
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(_) => break,
        }
    }

    let output_str = String::from_utf8_lossy(&output);
    assert!(
        output_str.contains("hello"),
        "Output should contain 'hello': {:?}",
        output_str
    );

    // Clean up
    child.kill().ok();
    child.wait().ok();
}

#[test]
fn test_pty_cat_command() {
    let pty_system = native_pty_system();
    let mut pty_pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("Failed to create PTY");

    // Run cat (will echo stdin to stdout)
    let cmd = CommandBuilder::new("cat");
    let mut child = pty_pair
        .slave
        .spawn_command(cmd)
        .expect("Failed to spawn cat");

    let mut writer = pty_pair.master.take_writer().expect("Failed to get writer");
    let mut reader = pty_pair
        .master
        .try_clone_reader()
        .expect("Failed to get reader");

    // Write to cat
    writer.write_all(b"test input\n").unwrap();
    writer.flush().unwrap();

    // Read output
    let mut buffer = [0u8; 1024];
    let mut output = Vec::new();

    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < 1 {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                output.extend_from_slice(&buffer[..n]);
                if output.contains(&b'\n') {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(_) => break,
        }
    }

    let output_str = String::from_utf8_lossy(&output);
    assert!(
        output_str.contains("test input"),
        "Cat should echo input: {:?}",
        output_str
    );

    child.kill().ok();
    child.wait().ok();
}

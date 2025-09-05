use std::sync::{Arc, Mutex};
use std::io::{Read, Write};
use std::fs::OpenOptions;
use tokio::task::JoinHandle;
use crate::server::terminal_state::TerminalBackend;

/// Handle stdin to PTY writing (byte by byte for raw mode)
pub fn spawn_stdin_reader(
    master_writer: Arc<Mutex<Option<Box<dyn std::io::Write + Send>>>>,
    debug_recorder_path: Option<String>
) -> JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        // Open debug recording file if specified
        let mut debug_file = debug_recorder_path.and_then(|path| {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .ok()
                .map(|f| {
                    // Recording stdin silently
                    f
                })
        });

        let mut stdin = std::io::stdin();
        let mut buffer = [0u8; 1];
        
        loop {
            match stdin.read_exact(&mut buffer) {
                Ok(_) => {
                    // Record to debug file if enabled
                    if let Some(ref mut file) = debug_file {
                        let _ = file.write_all(&buffer);
                        let _ = file.flush();
                    }

                    // Write single byte to PTY
                    let mut writer_guard = master_writer.lock().unwrap();
                    if let Some(writer) = writer_guard.as_mut() {
                        if let Err(_) = writer.write_all(&buffer) {
                            // Error writing to PTY - exit cleanly
                            break;
                        }
                        let _ = writer.flush();
                    } else {
                        break;
                    }
                }
                Err(_) => {
                    // Error or EOF on stdin - exit cleanly
                    break;
                }
            }
        }
    })
}

/// Handle PTY to stdout reading with terminal state tracking
pub fn spawn_pty_reader_with_tracker(
    master_reader: Arc<Mutex<Option<Box<dyn std::io::Read + Send>>>>,
    terminal_backend: Arc<Mutex<Box<dyn TerminalBackend>>>,
    debug_recorder_path: Option<String>
) -> JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        // Open debug recording file if specified
        let mut debug_file = debug_recorder_path.and_then(|path| {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .ok()
                .map(|f| {
                    // Recording PTY output silently
                    f
                })
        });

        let mut buffer = [0u8; 4096];
        loop {
            // Read from PTY (scope the lock)
            let read_result = {
                let mut reader_guard = master_reader.lock().unwrap();
                if let Some(reader) = reader_guard.as_mut() {
                    // Read from PTY
                    match reader.read(&mut buffer) {
                        Ok(0) => Some(Err("EOF".to_string())),
                        Ok(n) => Some(Ok(buffer[..n].to_vec())),
                        Err(e) => Some(Err(e.to_string())),
                    }
                } else {
                    None
                }
            }; // Lock is dropped here
            
            // Process the result outside the lock
            match read_result {
                Some(Ok(data)) => {
                    // Record to debug file if enabled (raw bytes)
                    if let Some(ref mut file) = debug_file {
                        let _ = file.write_all(&data);
                        let _ = file.flush();
                    }

                    // Update terminal backend
                    {
                        let mut backend = terminal_backend.lock().unwrap();
                        let _ = backend.process_output(&data);
                    }
                    
                    // Got data from PTY - write raw bytes to stdout to preserve UTF-8
                    use std::io::Write;
                    let _ = std::io::stdout().write_all(&data);
                    let _ = std::io::stdout().flush();
                }
                Some(Err(_e)) => {
                    // Error or EOF - exit
                    break;
                }
                None => {
                    // Reader has been removed, exit
                    break;
                }
            }
            
            // Small yield to prevent tight loop
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    })
}
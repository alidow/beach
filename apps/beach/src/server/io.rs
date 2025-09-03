use std::sync::{Arc, Mutex};
use std::io::{Read, Write};
use tokio::task::JoinHandle;
use crate::server::terminal_state::TerminalStateTracker;

/// Handle PTY to stdout reading
pub fn spawn_pty_reader(
    master_reader: Arc<Mutex<Option<Box<dyn std::io::Read + Send>>>>
) -> JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
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
                    // Got data from PTY - write directly to stdout
                    // TODO: Send to transport
                    print!("{}", String::from_utf8_lossy(&data));
                    std::io::stdout().flush().unwrap();
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

/// Handle stdin to PTY writing (byte by byte for raw mode)
pub fn spawn_stdin_reader(
    master_writer: Arc<Mutex<Option<Box<dyn std::io::Write + Send>>>>
) -> JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        let mut stdin = std::io::stdin();
        let mut buffer = [0u8; 1];
        
        loop {
            match stdin.read_exact(&mut buffer) {
                Ok(_) => {
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
    terminal_tracker: Arc<Mutex<TerminalStateTracker>>
) -> JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
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
                    // Update terminal state tracker
                    {
                        let mut tracker = terminal_tracker.lock().unwrap();
                        tracker.process_output(&data);
                    }
                    
                    // Got data from PTY - write directly to stdout
                    print!("{}", String::from_utf8_lossy(&data));
                    std::io::stdout().flush().unwrap();
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
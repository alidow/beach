use std::sync::{Arc, Mutex};
use std::io::{Read, Write};
use std::fs::OpenOptions;
use tokio::task::JoinHandle;
use crossterm::terminal;
use crate::server::terminal_state::TerminalBackend;
use crate::server::pty::PtyManager;

#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(unix)]
use signal_hook::consts::SIGWINCH;
#[cfg(unix)]
use signal_hook::flag;

#[cfg(windows)]
use crossterm::event::{poll, read, Event};
#[cfg(windows)]
use std::time::Duration;

/// Handle PTY reading with terminal resize support (cross-platform)
pub fn spawn_pty_reader_with_resize(
    master_reader: Arc<Mutex<Option<Box<dyn std::io::Read + Send>>>>,
    terminal_backend: Arc<Mutex<Box<dyn TerminalBackend>>>,
    pty_manager: Arc<PtyManager>,
    debug_recorder_path: Option<String>
) -> JoinHandle<()> {
    // Platform-specific resize detection setup
    #[cfg(unix)]
    let resize_pending = {
        // Create an Arc<AtomicBool> for resize signaling
        let flag = Arc::new(AtomicBool::new(false));
        // Try to register SIGWINCH handler to set atomic flag
        // If it fails, just continue without resize support
        let _ = flag::register(SIGWINCH, Arc::clone(&flag));
        flag
    };
    
    tokio::task::spawn_blocking(move || {
        // Open debug recording file if specified
        let mut debug_file = debug_recorder_path.and_then(|path| {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .ok()
                .map(|f| {
                    // Debug recording silently
                    f
                })
        });

        let mut buffer = [0u8; 4096];
        
        loop {
            // CRITICAL: Check for resize BEFORE reading next chunk
            #[cfg(unix)]
            {
                if resize_pending.load(Ordering::SeqCst) {
                    resize_pending.store(false, Ordering::SeqCst);
                    handle_resize(&terminal_backend, &pty_manager);
                }
            }
            
            #[cfg(windows)]
            {
                // On Windows, poll for resize events with zero timeout (non-blocking)
                if let Ok(true) = poll(Duration::from_millis(0)) {
                    if let Ok(Event::Resize(cols, rows)) = read() {
                        // Resize PTY and backend
                        if let Err(_e) = pty_manager.resize(cols, rows) {
                            // Silently handle resize error
                        }
                        
                        // Resize backend (preserves content, records delta)
                        {
                            let mut backend = terminal_backend.lock().unwrap();
                            if let Err(_e) = backend.resize(cols, rows) {
                                // Silently handle resize error
                            }
                        }
                    }
                }
            }
            
            // Read from PTY
            let read_result = {
                let mut reader_guard = master_reader.lock().unwrap();
                if let Some(reader) = reader_guard.as_mut() {
                    match reader.read(&mut buffer) {
                        Ok(0) => Some(Err("EOF".to_string())),
                        Ok(n) => Some(Ok(buffer[..n].to_vec())),
                        Err(e) => Some(Err(e.to_string())),
                    }
                } else {
                    None
                }
            };
            
            // Process the result
            match read_result {
                Some(Ok(data)) => {
                    // Record to debug file if enabled
                    if let Some(ref mut file) = debug_file {
                        let _ = file.write_all(&data);
                        let _ = file.flush();
                    }
                    
                    // Update terminal backend
                    {
                        let mut backend = terminal_backend.lock().unwrap();
                        let _ = backend.process_output(&data);
                    }
                    
                    // Write raw bytes to stdout to preserve UTF-8 sequences
                    use std::io::Write;
                    let _ = std::io::stdout().write_all(&data);
                    let _ = std::io::stdout().flush();
                }
                Some(Err(_e)) => {
                    // Error or EOF - exit
                    break;
                }
                None => {
                    // Reader has been removed
                    break;
                }
            }
            
            // Small yield to prevent tight loop
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    })
}

// Helper function for Unix resize handling
#[cfg(unix)]
fn handle_resize(
    terminal_backend: &Arc<Mutex<Box<dyn TerminalBackend>>>,
    pty_manager: &Arc<PtyManager>
) {
    // Get new terminal size
    match terminal::size() {
        Ok((cols, rows)) => {
            // Resize PTY (sends SIGWINCH to child)
            if let Err(_e) = pty_manager.resize(cols, rows) {
                // Silently handle resize error
            }
            
            // Resize backend (preserves content, records delta)
            {
                let mut backend = terminal_backend.lock().unwrap();
                if let Err(_e) = backend.resize(cols, rows) {
                    // Silently handle resize error
                }
            }
        }
        Err(_e) => {
            // Silently handle terminal size error
        }
    }
}
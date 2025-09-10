use std::sync::{Arc, Mutex};
use std::io::{Read, Write};
use std::fs::OpenOptions;
use tokio::task::JoinHandle;
use tokio::sync::mpsc::Sender as TokioSender;
use crossterm::terminal;
use crate::server::terminal_state::{TerminalBackend, Grid, GridDelta};
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

// ==================== STDIN HANDLING ====================

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

// ==================== PTY OUTPUT WITH RESIZE HANDLING ====================

/// Handle PTY reading with terminal resize support (cross-platform)
pub fn spawn_pty_reader_with_resize(
    master_reader: Arc<Mutex<Option<Box<dyn std::io::Read + Send>>>>,
    terminal_backend: Arc<Mutex<Box<dyn TerminalBackend>>>,
    pty_manager: Arc<PtyManager>,
    debug_recorder_path: Option<String>,
    delta_tx: Option<TokioSender<GridDelta>>,
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
        // Keep last grid to compute deltas
        let mut last_grid: Option<Grid> = None;
        
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
                    
                    // Update terminal backend and publish delta if changed
                    let current_grid = {
                        let mut backend = terminal_backend.lock().unwrap();
                        let _ = backend.process_output(&data);
                        backend.get_current_grid()
                    };
                    if let Some(prev) = &last_grid {
                        let delta = GridDelta::diff(prev, &current_grid);
                        // Send only if meaningful changes exist
                        if !delta.cell_changes.is_empty() || delta.cursor_change.is_some() || delta.dimension_change.is_some() {
                            if let Some(ref tx) = delta_tx {
                                // Log to debug file
                                if let Some(ref mut f) = debug_file {
                                    let _ = writeln!(f, "[{}] [PTY Reader] Sending delta: {} cell changes, cursor: {:?}, dim: {:?}", 
                                        chrono::Local::now().format("%H:%M:%S%.3f"),
                                        delta.cell_changes.len(), delta.cursor_change.is_some(), delta.dimension_change.is_some());
                                }
                                match tx.try_send(delta) {
                                    Ok(_) => {
                                        if let Some(ref mut f) = debug_file {
                                            let _ = writeln!(f, "[{}] [PTY Reader] Delta sent successfully", 
                                                chrono::Local::now().format("%H:%M:%S%.3f"));
                                        }
                                    },
                                    Err(e) => {
                                        if let Some(ref mut f) = debug_file {
                                            let _ = writeln!(f, "[{}] [PTY Reader] Failed to send delta: {:?}", 
                                                chrono::Local::now().format("%H:%M:%S%.3f"), e);
                                        }
                                    },
                                }
                            } else {
                                if let Some(ref mut f) = debug_file {
                                    let _ = writeln!(f, "[{}] [PTY Reader] WARNING: No delta_tx channel available!", 
                                        chrono::Local::now().format("%H:%M:%S%.3f"));
                                }
                            }
                        }
                    }
                    last_grid = Some(current_grid);
                    
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

// ==================== HELPER FUNCTIONS ====================

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

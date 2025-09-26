use anyhow::Result;
use async_trait::async_trait;
use portable_pty::{Child, MasterPty, PtySize};
use std::sync::{Arc, Mutex};

use crate::protocol::Dimensions;
use crate::server::pty::PtyManager;
use crate::subscription::PtyWriter;

/// Implementation of PtyWriter that wraps a PTY
pub struct PtyWriterImpl {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn std::io::Write + Send>>>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
}

impl PtyWriterImpl {
    pub fn new(
        master: Box<dyn MasterPty + Send>,
        child: Box<dyn Child + Send + Sync>,
    ) -> Result<Self> {
        let writer = master.take_writer()?;
        Ok(Self {
            master: Arc::new(Mutex::new(master)),
            writer: Arc::new(Mutex::new(writer)),
            child: Arc::new(Mutex::new(child)),
        })
    }
}

#[async_trait]
impl PtyWriter for PtyWriterImpl {
    async fn write(&self, bytes: &[u8]) -> Result<()> {
        use std::io::Write;
        let mut writer = self.writer.lock().unwrap();
        writer.write_all(bytes)?;
        writer.flush()?;
        Ok(())
    }

    async fn resize(&self, dims: Dimensions) -> Result<()> {
        let master = self.master.lock().unwrap();
        let size = PtySize {
            rows: dims.height,
            cols: dims.width,
            pixel_width: 0,
            pixel_height: 0,
        };
        master.resize(size)?;
        Ok(())
    }
}

/// Simple adapter to use PtyManager directly as a PtyWriter backend
pub struct PtyWriterFromManager {
    manager: PtyManager,
    debug_log_path: Option<String>,
}

impl PtyWriterFromManager {
    pub fn new(manager: PtyManager) -> Self {
        // Check for debug log from environment or use default
        let debug_log_path = std::env::var("BEACH_DEBUG_LOG").ok();
        Self {
            manager,
            debug_log_path,
        }
    }

    pub fn new_with_debug(manager: PtyManager, debug_log_path: Option<String>) -> Self {
        Self {
            manager,
            debug_log_path,
        }
    }
}

#[async_trait]
impl PtyWriter for PtyWriterFromManager {
    async fn write(&self, bytes: &[u8]) -> Result<()> {
        // Log before PTY write
        let write_start = std::time::Instant::now();
        if let Some(ref path) = self.debug_log_path {
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                use std::io::Write;
                let _ = writeln!(
                    f,
                    "[{}] [PtyWriter] Writing {} bytes to PTY",
                    chrono::Local::now().format("%H:%M:%S%.3f"),
                    bytes.len()
                );
                // Log the actual bytes for debugging (limit to first 50 chars)
                let preview = String::from_utf8_lossy(&bytes[..bytes.len().min(50)]);
                let _ = writeln!(
                    f,
                    "[{}] [PtyWriter] Data preview: {:?}",
                    chrono::Local::now().format("%H:%M:%S%.3f"),
                    preview
                );
            }
        }

        let result = self.manager.write(bytes);
        let elapsed = write_start.elapsed();

        // Log after PTY write with timing
        if let Some(ref path) = self.debug_log_path {
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                use std::io::Write;
                match &result {
                    Ok(_) => {
                        let _ = writeln!(
                            f,
                            "[{}] [PtyWriter] Write successful, took {}μs",
                            chrono::Local::now().format("%H:%M:%S%.3f"),
                            elapsed.as_micros()
                        );
                    }
                    Err(e) => {
                        let _ = writeln!(
                            f,
                            "[{}] [PtyWriter] Write failed after {}μs: {:?}",
                            chrono::Local::now().format("%H:%M:%S%.3f"),
                            elapsed.as_micros(),
                            e
                        );
                    }
                }
            }
        }

        result
    }

    async fn resize(&self, dims: Dimensions) -> Result<()> {
        self.manager.resize(dims.width, dims.height)
    }
}

use portable_pty::{CommandBuilder, PtySize, native_pty_system, Child, PtyPair};
use std::sync::{Arc, Mutex};
use anyhow::Result;

/// Manages PTY lifecycle and operations
#[derive(Clone)]
pub struct PtyManager {
    pub pty_pair: Arc<Mutex<Option<PtyPair>>>,
    pub master_writer: Arc<Mutex<Option<Box<dyn std::io::Write + Send>>>>,
    pub master_reader: Arc<Mutex<Option<Box<dyn std::io::Read + Send>>>>,
    pub child: Arc<Mutex<Option<Box<dyn Child + Send + Sync>>>>,
}

impl PtyManager {
    pub fn new() -> Self {
        Self {
            pty_pair: Arc::new(Mutex::new(None)),
            master_writer: Arc::new(Mutex::new(None)),
            master_reader: Arc::new(Mutex::new(None)),
            child: Arc::new(Mutex::new(None)),
        }
    }

    /// Initialize PTY with given size and command
    pub fn init(&self, pty_size: PtySize, cmd: CommandBuilder) -> Result<()> {
        // Create PTY system
        let pty_system = native_pty_system();
        
        // Create PTY pair with size
        let pty_pair = pty_system.openpty(pty_size)?;
        
        // Spawn the command in the PTY
        let child = pty_pair.slave.spawn_command(cmd)?;
        
        // Get reader and writer from master
        let master_reader = pty_pair.master.try_clone_reader()?;
        let master_writer = pty_pair.master.take_writer()?;
        
        // Store everything
        *self.child.lock().unwrap() = Some(child);
        *self.pty_pair.lock().unwrap() = Some(pty_pair);
        *self.master_writer.lock().unwrap() = Some(master_writer);
        *self.master_reader.lock().unwrap() = Some(master_reader);
        
        Ok(())
    }

    /// Write data to the PTY
    pub fn write(&self, data: &[u8]) -> Result<()> {
        let mut writer_guard = self.master_writer.lock().unwrap();
        if let Some(writer) = writer_guard.as_mut() {
            writer.write_all(data)?;
            writer.flush()?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("PTY writer not available"))
        }
    }

    /// Resize the PTY
    pub fn resize(&self, width: u16, height: u16) -> Result<()> {
        let pty_pair_guard = self.pty_pair.lock().unwrap();
        if let Some(pty_pair) = pty_pair_guard.as_ref() {
            let pty_size = PtySize {
                rows: height,
                cols: width,
                pixel_width: 0,
                pixel_height: 0,
            };
            pty_pair.master.resize(pty_size)?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("PTY not initialized"))
        }
    }

    /// Clean up PTY resources
    pub fn cleanup(&self) {
        // Kill the child process
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        
        // Clear all resources
        *self.pty_pair.lock().unwrap() = None;
        *self.master_writer.lock().unwrap() = None;
        *self.master_reader.lock().unwrap() = None;
    }
}

impl Drop for PtyManager {
    fn drop(&mut self) {
        self.cleanup();
    }
}
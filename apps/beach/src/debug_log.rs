use std::sync::{Arc, Mutex};
use std::fs::File;
use std::io::Write;
use chrono::Utc;

/// Thread-safe debug logger that writes to file when available
#[derive(Clone)]
pub struct DebugLogger {
    file: Arc<Mutex<Option<File>>>,
    verbose: bool,
}

impl DebugLogger {
    /// Create a new debug logger
    pub fn new(debug_file: Option<File>) -> Self {
        Self {
            file: Arc::new(Mutex::new(debug_file)),
            verbose: std::env::var("BEACH_VERBOSE").is_ok(),
        }
    }
    
    /// Log a debug message (only when verbose mode is enabled)
    pub fn log(&self, message: &str) {
        if !self.verbose {
            return;
        }
        
        if let Ok(mut file_guard) = self.file.lock() {
            if let Some(ref mut file) = *file_guard {
                let timestamp = Utc::now().format("%Y-%m-%d %H:%M:%S%.3f");
                let _ = writeln!(file, "[{}] {}", timestamp, message);
                let _ = file.flush();
            }
        }
    }
    
    /// Log an error message (always logged)
    pub fn error(&self, message: &str) {
        if let Ok(mut file_guard) = self.file.lock() {
            if let Some(ref mut file) = *file_guard {
                let timestamp = Utc::now().format("%Y-%m-%d %H:%M:%S%.3f");
                let _ = writeln!(file, "[{}] ERROR: {}", timestamp, message);
                let _ = file.flush();
            }
        }
    }
    
    /// Check if verbose mode is enabled
    pub fn is_verbose(&self) -> bool {
        self.verbose
    }
}

/// Macro for debug logging that respects --debug-log
#[macro_export]
macro_rules! debug_log {
    ($logger:expr, $($arg:tt)*) => {
        if let Some(ref logger) = $logger {
            logger.log(&format!($($arg)*));
        }
    };
}

/// Macro for error logging that respects --debug-log
#[macro_export]
macro_rules! error_log {
    ($logger:expr, $($arg:tt)*) => {
        if let Some(ref logger) = $logger {
            logger.error(&format!($($arg)*));
        }
    };
}
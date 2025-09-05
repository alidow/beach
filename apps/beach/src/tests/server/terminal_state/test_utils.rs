use crate::server::terminal_state::{TerminalBackend, create_terminal_backend, Grid, GridHistory};
use std::sync::{Arc, Mutex};

/// Create a test terminal backend based on features
pub fn create_test_terminal(width: u16, height: u16) -> Box<dyn TerminalBackend> {
    create_terminal_backend(width, height, None).expect("Failed to create terminal backend")
}

/// Helper struct to work with any backend in tests
pub struct TestTerminal {
    backend: Box<dyn TerminalBackend>,
}

impl TestTerminal {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            backend: create_test_terminal(width, height),
        }
    }
    
    pub fn process_output(&mut self, data: &[u8]) {
        self.backend.process_output(data).expect("Failed to process output");
    }
    
    pub fn force_snapshot(&mut self) {
        self.backend.force_snapshot();
    }
    
    pub fn get_history(&self) -> Arc<Mutex<GridHistory>> {
        self.backend.get_history()
    }
    
    pub fn get_current_grid(&self) -> Grid {
        self.backend.get_current_grid()
    }
}
use std::sync::{Arc, Mutex};
use crate::server::terminal_state::{Grid, GridHistory};

/// Trait for terminal backend implementations
/// 
/// This allows us to swap between different terminal emulation libraries
/// (vte-based custom implementation vs alacritty_terminal) while maintaining
/// the same interface.
pub trait TerminalBackend: Send + Sync {
    /// Process terminal output data (ANSI escape sequences and text)
    fn process_output(&mut self, data: &[u8]) -> anyhow::Result<()>;
    
    /// Get the current grid state
    fn get_current_grid(&self) -> Grid;
    
    /// Get the history of grid changes
    fn get_history(&self) -> Arc<Mutex<GridHistory>>;
    
    /// Force a snapshot for testing purposes
    fn force_snapshot(&mut self);
    
    /// Get terminal dimensions
    fn get_dimensions(&self) -> (u16, u16);
    
    /// Resize the terminal
    fn resize(&mut self, width: u16, height: u16) -> anyhow::Result<()>;
}

/// Factory function to create the appropriate backend based on features
pub fn create_terminal_backend(width: u16, height: u16, debug_log: Option<&std::fs::File>) -> anyhow::Result<Box<dyn TerminalBackend>> {
    use crate::server::terminal_state::AlacrittyTerminal;
    Ok(Box::new(AlacrittyTerminal::new(width, height, debug_log)?))
}

// Implement TerminalBackend for AlacrittyTerminal
impl TerminalBackend for crate::server::terminal_state::AlacrittyTerminal {
    fn process_output(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.process_output(data)
    }
    
    fn get_current_grid(&self) -> Grid {
        self.get_current_grid()
    }
    
    fn get_history(&self) -> Arc<Mutex<GridHistory>> {
        self.get_history()
    }
    
    fn force_snapshot(&mut self) {
        self.force_snapshot();
    }
    
    fn get_dimensions(&self) -> (u16, u16) {
        let grid = self.get_current_grid();
        (grid.width, grid.height)
    }
    
    fn resize(&mut self, width: u16, height: u16) -> anyhow::Result<()> {
        self.resize(width, height)
    }
}

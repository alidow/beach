use std::sync::{Arc, Mutex};
use chrono::{DateTime, Duration, Utc};
use vte::Parser;
use crate::server::terminal_state::{Grid, GridHistory, GridDelta, GridUpdater, TerminalInitializer};

pub struct TerminalStateTracker {
    current_grid: Grid,
    history: Arc<Mutex<GridHistory>>,
    parser: Parser,
    last_update: DateTime<Utc>,
    update_interval: Duration,
    previous_grid: Option<Grid>,
}

impl TerminalStateTracker {
    /// Create a new tracker with environment-aware initial state
    /// 
    /// This uses TerminalInitializer to detect terminal colors from environment
    /// variables and create a grid that better matches the terminal's appearance
    pub fn new(width: u16, height: u16) -> Self {
        // Create initial grid with terminal-aware defaults
        let initial_grid = TerminalInitializer::create_initial_grid(width, height);
        Self::with_initial_grid(initial_grid)
    }
    
    /// Create a new tracker with a custom initial grid
    pub fn with_initial_grid(initial_grid: Grid) -> Self {
        let history = Arc::new(Mutex::new(GridHistory::new(initial_grid.clone())));
        
        TerminalStateTracker {
            current_grid: initial_grid.clone(),
            history,
            parser: Parser::new(),
            last_update: Utc::now(),
            update_interval: Duration::milliseconds(50),
            previous_grid: Some(initial_grid),
        }
    }
    
    pub fn process_output(&mut self, data: &[u8]) {
        // Clone the current grid state before modifying
        let old_grid = self.current_grid.clone();
        
        // Update timestamp for new grid state
        self.current_grid.timestamp = Utc::now();
        
        // Parse ANSI sequences and update grid
        let mut updater = GridUpdater::new(&mut self.current_grid);
        
        for byte in data {
            self.parser.advance(&mut updater, *byte);
        }
        
        // Always create a delta after processing to track changes
        // This ensures test assertions can see the changes
        let now = Utc::now();
        let should_create_delta = if let Some(prev) = &self.previous_grid {
            // Check if anything actually changed
            &old_grid != prev || self.current_grid != old_grid
        } else {
            true
        };
        
        if should_create_delta {
            // Create delta comparing old state to new state
            let delta = GridDelta::diff(&old_grid, &self.current_grid);
            
            // Only add delta if there are actual changes or enough time has passed
            if !delta.cell_changes.is_empty() || 
               delta.cursor_change.is_some() || 
               delta.dimension_change.is_some() ||
               now - self.last_update > self.update_interval {
                
                let mut history = self.history.lock().unwrap();
                history.add_delta(delta);
                self.last_update = now;
                self.previous_grid = Some(self.current_grid.clone());
            }
        }
    }
    
    fn create_delta(&mut self) {
        let mut history = self.history.lock().unwrap();
        let previous = self.previous_grid.as_ref().unwrap_or(&self.current_grid);
        let delta = GridDelta::diff(previous, &self.current_grid);
        if !delta.cell_changes.is_empty() || delta.cursor_change.is_some() || delta.dimension_change.is_some() {
            history.add_delta(delta);
            self.previous_grid = Some(self.current_grid.clone());
        }
    }
    
    pub fn get_history(&self) -> Arc<Mutex<GridHistory>> {
        Arc::clone(&self.history)
    }
    
    // Force a snapshot for testing
    pub fn force_snapshot(&mut self) {
        self.create_delta();
    }
}

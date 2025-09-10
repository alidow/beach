use std::sync::{Arc, Mutex};
use chrono::{DateTime, Duration, Utc};
#[cfg(feature = "vte-backend")]
use vte::Parser;
use crate::server::terminal_state::{Grid, GridHistory, GridDelta, TerminalInitializer, Color, CellAttributes, TerminalBackend, create_terminal_backend};
#[cfg(feature = "vte-backend")]
use crate::server::terminal_state::GridUpdater;

pub struct TerminalStateTracker {
    current_grid: Grid,
    history: Arc<Mutex<GridHistory>>,
    #[cfg(feature = "vte-backend")]
    parser: Parser,
    backend: Option<Arc<Mutex<Box<dyn TerminalBackend>>>>,
    last_update: DateTime<Utc>,
    update_interval: Duration,
    previous_grid: Option<Grid>,
    // SGR state that persists across process_output calls
    current_fg: Color,
    current_bg: Color,
    current_attrs: CellAttributes,
}

impl TerminalStateTracker {
    /// Create a new tracker with environment-aware initial state
    /// 
    /// This uses TerminalInitializer to detect terminal colors from environment
    /// variables and create a grid that better matches the terminal's appearance
    pub fn new(width: u16, height: u16) -> Self {
        // Create the terminal backend
        let backend = create_terminal_backend(width, height, None).ok()
            .map(|b| Arc::new(Mutex::new(b)));
        
        // Get the initial grid from backend if available, otherwise create default
        let initial_grid = if let Some(ref backend) = backend {
            backend.lock().unwrap().get_current_grid()
        } else {
            TerminalInitializer::create_initial_grid(width, height)
        };
        
        // Create history only if backend doesn't provide one
        let history = if backend.is_none() {
            Arc::new(Mutex::new(GridHistory::new(initial_grid.clone())))
        } else {
            // Dummy history - we'll use the backend's
            Arc::new(Mutex::new(GridHistory::new(initial_grid.clone())))
        };
        
        TerminalStateTracker {
            current_grid: initial_grid.clone(),
            history,
            #[cfg(feature = "vte-backend")]
            parser: Parser::new(),
            backend,
            last_update: Utc::now(),
            update_interval: Duration::milliseconds(50),
            previous_grid: Some(initial_grid.clone()),
            current_fg: Color::Default,
            current_bg: Color::Default,
            current_attrs: CellAttributes::default(),
        }
    }
    
    /// Create a new tracker with a custom initial grid
    pub fn with_initial_grid(initial_grid: Grid) -> Self {
        let history = Arc::new(Mutex::new(GridHistory::new(initial_grid.clone())));
        
        TerminalStateTracker {
            current_grid: initial_grid.clone(),
            history,
            #[cfg(feature = "vte-backend")]
            parser: Parser::new(),
            backend: None,
            last_update: Utc::now(),
            update_interval: Duration::milliseconds(50),
            previous_grid: Some(initial_grid),
            current_fg: Color::Default,
            current_bg: Color::Default,
            current_attrs: CellAttributes::default(),
        }
    }
    
    /// Create a new tracker that uses an existing backend for history
    pub fn from_backend(backend: Arc<Mutex<Box<dyn TerminalBackend>>>) -> Self {
        // Get initial grid and dimensions from the backend
        let backend_box = backend.lock().unwrap();
        let initial_grid = backend_box.get_current_grid();
        drop(backend_box); // Release lock early
        
        // Create dummy history - we'll use the backend's history
        let history = Arc::new(Mutex::new(GridHistory::new(initial_grid.clone())));
        
        TerminalStateTracker {
            current_grid: initial_grid.clone(),
            history,
            #[cfg(feature = "vte-backend")]
            parser: Parser::new(),
            backend: Some(backend),
            last_update: Utc::now(),
            update_interval: Duration::milliseconds(50),
            previous_grid: Some(initial_grid),
            current_fg: Color::Default,
            current_bg: Color::Default,
            current_attrs: CellAttributes::default(),
        }
    }
    
    pub fn process_output(&mut self, data: &[u8]) {
        // Use backend if available
        if let Some(ref backend) = self.backend {
            let mut backend = backend.lock().unwrap();
            let _ = backend.process_output(data);
            // Update our cached grid
            self.current_grid = backend.get_current_grid();
            // History is already updated by the backend
            return;
        }
        
        #[cfg(feature = "vte-backend")]
        {
            // Clone the current grid state before modifying
            let old_grid = self.current_grid.clone();
            
            // Update timestamp for new grid state
            self.current_grid.timestamp = Utc::now();
            
            // Parse ANSI sequences and update grid with persistent SGR state
            let mut updater = GridUpdater::new_with_state(
                &mut self.current_grid, 
                self.current_fg.clone(), 
                self.current_bg.clone(), 
                self.current_attrs.clone()
            );
            
            for byte in data {
                self.parser.advance(&mut updater, *byte);
            }
            
            // Save the SGR state back to the tracker for next call
            self.current_fg = updater.current_fg.clone();
            self.current_bg = updater.current_bg.clone();
            self.current_attrs = updater.current_attrs.clone();
            
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
        
        #[cfg(not(feature = "vte-backend"))]
        {
            // No-op when vte backend is disabled
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
        // Always prefer backend's history if available
        if let Some(ref backend) = self.backend {
            return backend.lock().unwrap().get_history();
        }
        Arc::clone(&self.history)
    }
    
    // Force a snapshot for testing
    pub fn force_snapshot(&mut self) {
        self.create_delta();
    }
}

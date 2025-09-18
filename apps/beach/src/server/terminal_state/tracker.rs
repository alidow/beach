use crate::server::terminal_state::{
    Grid, GridDelta, GridHistory, TerminalBackend, TerminalInitializer, create_terminal_backend,
};
use std::sync::{Arc, Mutex};

pub struct TerminalStateTracker {
    current_grid: Grid,
    history: Arc<Mutex<GridHistory>>,
    backend: Option<Arc<Mutex<Box<dyn TerminalBackend>>>>,
    previous_grid: Option<Grid>,
}

impl TerminalStateTracker {
    /// Create a new tracker with environment-aware initial state
    ///
    /// This uses TerminalInitializer to detect terminal colors from environment
    /// variables and create a grid that better matches the terminal's appearance
    pub fn new(width: u16, height: u16) -> Self {
        // Create the terminal backend
        let backend = create_terminal_backend(width, height, None, None)
            .ok()
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
            backend,
            previous_grid: Some(initial_grid.clone()),
        }
    }

    /// Create a new tracker with a custom initial grid
    pub fn with_initial_grid(initial_grid: Grid) -> Self {
        let history = Arc::new(Mutex::new(GridHistory::new(initial_grid.clone())));

        TerminalStateTracker {
            current_grid: initial_grid.clone(),
            history,
            backend: None,
            previous_grid: Some(initial_grid),
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
            backend: Some(backend),
            previous_grid: Some(initial_grid),
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

        // No-op when no backend is available - vte-backend has been removed
    }

    fn create_delta(&mut self) {
        let mut history = self.history.lock().unwrap();
        let previous = self.previous_grid.as_ref().unwrap_or(&self.current_grid);
        let delta = GridDelta::diff(previous, &self.current_grid);
        if !delta.cell_changes.is_empty()
            || delta.cursor_change.is_some()
            || delta.dimension_change.is_some()
        {
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

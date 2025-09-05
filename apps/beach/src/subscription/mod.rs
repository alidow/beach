use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use anyhow::{Result, anyhow};

use crate::protocol::{
    ClientMessage, ServerMessage, ViewMode, ViewPosition, Dimensions,
    ErrorCode, SubscriptionStatus, SubscriptionInfo
};
use crate::server::terminal_state::{Grid, GridDelta, GridView, TerminalStateTracker};
use crate::transport::Transport;

pub type SubscriptionId = String;
pub type ClientId = String;

/// Represents a single client subscription to terminal output
pub struct Subscription {
    pub id: SubscriptionId,
    pub client_id: ClientId,
    pub dimensions: Dimensions,
    pub mode: ViewMode,
    pub position: Option<ViewPosition>,
    pub grid_view: Arc<Mutex<GridView>>,
    pub connection: mpsc::Sender<ServerMessage>,
    pub is_controlling: bool,
    pub last_sequence_acked: u64,
    pub current_sequence: u64,
}

impl Subscription {
    pub fn new(
        id: SubscriptionId,
        client_id: ClientId,
        dimensions: Dimensions,
        mode: ViewMode,
        position: Option<ViewPosition>,
        connection: mpsc::Sender<ServerMessage>,
        is_controlling: bool,
        terminal_tracker: Arc<Mutex<TerminalStateTracker>>,
    ) -> Self {
        let history = terminal_tracker.lock().unwrap().get_history();
        let grid_view = Arc::new(Mutex::new(GridView::new(history)));
        
        Self {
            id,
            client_id,
            dimensions,
            mode,
            position,
            grid_view,
            connection,
            is_controlling,
            last_sequence_acked: 0,
            current_sequence: 0,
        }
    }
    
    /// Send a message to the client
    pub async fn send(&self, message: ServerMessage) -> Result<()> {
        self.connection.send(message).await
            .map_err(|e| anyhow!("Failed to send message: {}", e))
    }
    
    /// Create a snapshot of the current grid state
    pub fn create_snapshot(&self) -> Result<Grid> {
        let grid = self.grid_view.lock().unwrap()
            .derive_realtime(Some(self.dimensions.height))?;
        Ok(grid)
    }
    
    /// Update dimensions for this subscription
    pub fn update_dimensions(&mut self, dimensions: Dimensions) {
        self.dimensions = dimensions;
    }
    
    /// Update view mode and position
    pub fn update_view(&mut self, mode: ViewMode, position: Option<ViewPosition>) {
        self.mode = mode;
        self.position = position;
    }
    
    /// Increment and return the current sequence number
    pub fn next_sequence(&mut self) -> u64 {
        self.current_sequence += 1;
        self.current_sequence
    }
}

pub mod manager;
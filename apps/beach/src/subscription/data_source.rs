use async_trait::async_trait;
use anyhow::Result;
use crate::protocol::Dimensions;
use crate::server::terminal_state::{Grid, GridDelta};

/// Trait for providing terminal data to the subscription system
/// This decouples SubscriptionHub from the specific terminal backend implementation
#[async_trait]
pub trait TerminalDataSource: Send + Sync {
    /// Get a snapshot of the current terminal state for the given dimensions
    async fn snapshot(&self, dims: Dimensions) -> Result<Grid>;
    
    /// Wait for and return the next terminal state change
    /// This should coalesce rapid changes for efficiency
    async fn next_delta(&self) -> Result<GridDelta>;
    
    /// Force a resync by invalidating any cached state
    async fn invalidate(&self) -> Result<()>;
}

/// Trait for writing to the PTY
#[async_trait]
pub trait PtyWriter: Send + Sync {
    /// Write bytes to the PTY
    async fn write(&self, bytes: &[u8]) -> Result<()>;
    
    /// Resize the PTY
    async fn resize(&self, dims: Dimensions) -> Result<()>;
}
use async_trait::async_trait;
use anyhow::Result;
use chrono::{DateTime, Utc};
use crate::protocol::{Dimensions, ViewMode, ViewPosition};
use crate::server::terminal_state::{Grid, GridDelta};

/// Trait for providing terminal data to the subscription system
/// This decouples SubscriptionHub from the specific terminal backend implementation
#[async_trait]
pub trait TerminalDataSource: Send + Sync {
    /// Get a snapshot of the current terminal state for the given dimensions
    async fn snapshot(&self, dims: Dimensions) -> Result<Grid>;
    
    /// Get a snapshot for a specific view mode and position
    /// DEPRECATED: Use snapshot_range_with_watermark for viewport-based access
    async fn snapshot_with_view(
        &self, 
        dims: Dimensions, 
        mode: ViewMode, 
        position: Option<ViewPosition>
    ) -> Result<Grid>;
    
    /// Get a snapshot for a specific line range with watermark sequence
    /// Returns a Grid for the requested range and the latest delta sequence
    /// that has been applied to this snapshot (watermark)
    async fn snapshot_range_with_watermark(
        &self,
        width: u16,
        start_line: u64,
        rows: u16,
    ) -> Result<(Grid, u64)>;
    
    /// Wait for and return the next terminal state change
    /// This should coalesce rapid changes for efficiency
    async fn next_delta(&self) -> Result<GridDelta>;
    
    /// Force a resync by invalidating any cached state
    async fn invalidate(&self) -> Result<()>;
    
    /// Get metadata about available history
    async fn get_history_metadata(&self) -> Result<HistoryMetadata>;
}

/// Metadata about available terminal history
#[derive(Clone, Debug)]
pub struct HistoryMetadata {
    /// Oldest available line number in history
    pub oldest_line: u64,
    /// Most recent line number
    pub latest_line: u64,
    /// Total number of lines in history
    pub total_lines: u64,
    /// Oldest available timestamp
    pub oldest_timestamp: Option<DateTime<Utc>>,
    /// Most recent timestamp
    pub latest_timestamp: Option<DateTime<Utc>>,
}

/// Trait for writing to the PTY
#[async_trait]
pub trait PtyWriter: Send + Sync {
    /// Write bytes to the PTY
    async fn write(&self, bytes: &[u8]) -> Result<()>;
    
    /// Resize the PTY
    async fn resize(&self, dims: Dimensions) -> Result<()>;
}
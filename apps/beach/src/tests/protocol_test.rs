#![recursion_limit = "1024"]

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use async_trait::async_trait;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    use crate::protocol::Dimensions;
    use crate::server::terminal_state::{Grid, GridDelta};
    use crate::subscription::{PtyWriter, SubscriptionHub, TerminalDataSource};
    use crate::transport::mock::MockTransport;

    // Mock implementation of TerminalDataSource for testing
    struct MockDataSource {
        grid: Arc<RwLock<Grid>>,
    }

    impl MockDataSource {
        fn new() -> Self {
            Self {
                grid: Arc::new(RwLock::new(Grid::new(80, 24))),
            }
        }
    }

    #[async_trait]
    impl TerminalDataSource for MockDataSource {
        async fn snapshot(&self, dims: Dimensions) -> Result<Grid> {
            let grid = self.grid.read().await;
            Ok(grid.clone())
        }

        async fn next_delta(&self) -> Result<GridDelta> {
            // For testing, just return an empty delta
            Ok(GridDelta {
                timestamp: chrono::Utc::now(),
                cell_changes: Vec::new(),
                dimension_change: None,
                cursor_change: None,
                sequence: 0,
            })
        }

        async fn invalidate(&self) -> Result<()> {
            Ok(())
        }

        async fn snapshot_with_view(
            &self,
            dims: Dimensions,
            _mode: crate::protocol::subscription::ViewMode,
            _position: Option<crate::protocol::subscription::ViewPosition>,
        ) -> Result<Grid> {
            // For testing, just return regular snapshot
            self.snapshot(dims).await
        }

        async fn snapshot_range_with_watermark(
            &self,
            width: u16,
            _start_line: u64,
            rows: u16,
        ) -> Result<(Grid, u64)> {
            // For testing, return a basic grid with watermark 0
            let grid = Grid::new(width, rows);
            Ok((grid, 0))
        }

        async fn get_history_metadata(&self) -> Result<crate::subscription::HistoryMetadata> {
            // For testing, return default metadata
            Ok(crate::subscription::HistoryMetadata {
                oldest_line: 0,
                latest_line: 100,
                total_lines: 100,
                oldest_timestamp: None,
                latest_timestamp: None,
            })
        }
    }

    // Mock implementation of PtyWriter for testing
    struct MockPtyWriter;

    #[async_trait]
    impl PtyWriter for MockPtyWriter {
        async fn write(&self, _bytes: &[u8]) -> Result<()> {
            Ok(())
        }

        async fn resize(&self, _dims: Dimensions) -> Result<()> {
            Ok(())
        }
    }

    #[test_timeout::tokio_timeout_test]
    async fn test_subscription_hub_creation() {
        let hub = SubscriptionHub::new();
        // Hub starts with no data source or writer
        // We can test this by trying to use them
    }

    #[test_timeout::tokio_timeout_test]
    async fn test_subscription_hub_attach_source() {
        let hub = SubscriptionHub::new();
        let source = Arc::new(MockDataSource::new());

        hub.attach_source(source.clone()).await;
        // Source is now attached and can be used
    }

    #[test_timeout::tokio_timeout_test]
    async fn test_subscription_hub_set_writer() {
        let hub = SubscriptionHub::new();
        let writer = Arc::new(MockPtyWriter);

        hub.set_pty_writer(writer.clone()).await;
        // Writer is now set and can be used
    }

    #[test_timeout::tokio_timeout_test]
    async fn test_subscription_creation() {
        use crate::protocol::subscription::{ViewMode, ViewPosition};
        use crate::subscription::SubscriptionConfig;

        let hub = SubscriptionHub::new();
        let source = Arc::new(MockDataSource::new());
        hub.attach_source(source).await;

        let transport = Arc::new(MockTransport::new());
        let config = SubscriptionConfig {
            dimensions: Dimensions {
                width: 80,
                height: 24,
            },
            mode: ViewMode::Realtime,
            position: Some(ViewPosition {
                time: None,
                line: None,
                offset: None,
            }),
            is_controlling: false,
            initial_fetch_size: None,
            stream_history: None,
        };

        let client_id = "test-client".to_string();
        let subscription_id = hub
            .subscribe(client_id, transport, config)
            .await
            .expect("Failed to create subscription");

        assert!(!subscription_id.is_empty());
    }
}

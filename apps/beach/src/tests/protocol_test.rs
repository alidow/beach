#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use async_trait::async_trait;
    use anyhow::Result;
    
    use crate::subscription::{SubscriptionHub, TerminalDataSource, PtyWriter};
    use crate::protocol::Dimensions;
    use crate::server::terminal_state::{Grid, GridDelta};
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
    
    #[tokio::test]
    async fn test_subscription_hub_creation() {
        let hub = SubscriptionHub::new();
        // Hub starts with no data source or writer
        // We can test this by trying to use them
    }
    
    #[tokio::test]
    async fn test_subscription_hub_attach_source() {
        let hub = SubscriptionHub::new();
        let source = Arc::new(MockDataSource::new());
        
        hub.attach_source(source.clone()).await;
        // Source is now attached and can be used
    }
    
    #[tokio::test]
    async fn test_subscription_hub_set_writer() {
        let hub = SubscriptionHub::new();
        let writer = Arc::new(MockPtyWriter);
        
        hub.set_pty_writer(writer.clone()).await;
        // Writer is now set and can be used
    }
    
    #[tokio::test]
    async fn test_subscription_creation() {
        use crate::subscription::SubscriptionConfig;
        use crate::protocol::subscription::{ViewMode, ViewPosition};
        
        let hub = SubscriptionHub::new();
        let source = Arc::new(MockDataSource::new());
        hub.attach_source(source).await;
        
        let transport = Arc::new(MockTransport::new());
        let config = SubscriptionConfig {
            dimensions: Dimensions { width: 80, height: 24 },
            mode: ViewMode::Realtime,
            position: Some(ViewPosition {
                time: None,
                line: None,
                offset: None,
            }),
            is_controlling: false,
        };
        
        let client_id = "test-client".to_string();
        let subscription_id = hub.subscribe(client_id, transport, config).await
            .expect("Failed to create subscription");
        
        assert!(!subscription_id.is_empty());
    }
}
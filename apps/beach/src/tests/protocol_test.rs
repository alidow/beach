#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc;
    
    use crate::protocol::{
        ClientMessage, ServerMessage, ViewMode, Dimensions, SubscriptionStatus
    };
    use crate::subscription::manager::SubscriptionManager;
    use crate::server::terminal_state::TerminalStateTracker;
    use crate::transport::mock::MockTransport;
    
    #[ignore] // TODO: Fix async test infrastructure
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_subscription_pooling() {
        let transport = MockTransport::new();
        let tracker = Arc::new(Mutex::new(TerminalStateTracker::new(80, 24)));
        let manager = Arc::new(SubscriptionManager::new(transport, tracker));
        
        let (tx1, mut rx1) = mpsc::channel::<ServerMessage>(100);
        let (tx2, mut rx2) = mpsc::channel::<ServerMessage>(100);
        
        let dimensions = Dimensions { width: 80, height: 24 };
        
        // Add subscription for client1
        manager.add_subscription(
            "sub1".to_string(),
            "client1".to_string(),
            dimensions.clone(),
            ViewMode::Realtime,
            None,
            tx1,
            false,
        ).await.unwrap();
        
        // First message should be SubscriptionAck
        if let Some(msg) = rx1.recv().await {
            match msg {
                ServerMessage::SubscriptionAck { status, shared_with, .. } => {
                    assert_eq!(status, SubscriptionStatus::Active);
                    assert!(shared_with.is_none());
                }
                _ => panic!("Expected SubscriptionAck, got {:?}", msg),
            }
        }
        
        // Second message should be Snapshot
        if let Some(msg) = rx1.recv().await {
            match msg {
                ServerMessage::Snapshot { .. } => {
                    // Expected snapshot
                }
                _ => panic!("Expected Snapshot, got {:?}", msg),
            }
        }
        
        // Add subscription for client2
        manager.add_subscription(
            "sub2".to_string(),
            "client2".to_string(),
            dimensions,
            ViewMode::Realtime,
            None,
            tx2,
            false,
        ).await.unwrap();
        
        // First message should be SubscriptionAck (now always Active, no sharing)
        if let Some(msg) = rx2.recv().await {
            match msg {
                ServerMessage::SubscriptionAck { status, shared_with, .. } => {
                    assert_eq!(status, SubscriptionStatus::Active);
                    assert!(shared_with.is_none());
                }
                _ => panic!("Expected SubscriptionAck, got {:?}", msg),
            }
        }
        
        // Second message should be Snapshot
        if let Some(msg) = rx2.recv().await {
            match msg {
                ServerMessage::Snapshot { .. } => {
                    // Expected snapshot
                }
                _ => panic!("Expected Snapshot, got {:?}", msg),
            }
        }
    }
    
    #[ignore] // TODO: Fix async test infrastructure
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_view_transition() {
        let transport = MockTransport::new();
        let tracker = Arc::new(Mutex::new(TerminalStateTracker::new(80, 24)));
        let manager = Arc::new(SubscriptionManager::new(transport, tracker));
        
        let (tx, mut rx) = mpsc::channel::<ServerMessage>(100);
        
        // Start with realtime view
        manager.add_subscription(
            "sub1".to_string(),
            "client1".to_string(),
            Dimensions { width: 80, height: 24 },
            ViewMode::Realtime,
            None,
            tx,
            false,
        ).await.unwrap();
        
        rx.recv().await;
        rx.recv().await;
        
        let modify = ClientMessage::ModifySubscription {
            subscription_id: "sub1".to_string(),
            dimensions: Some(Dimensions { width: 120, height: 40 }),
            mode: None,
            position: None,
        };
        
        manager.handle_client_message("client1".to_string(), modify).await.unwrap();
        
        if let Some(msg) = rx.recv().await {
            match msg {
                ServerMessage::Snapshot { .. } => {
                    // Modified subscription now sends a new snapshot
                }
                _ => panic!("Expected Snapshot after modification"),
            }
        }
    }
    
    // ViewKey tests removed - no longer needed with simplified architecture
    /*
    #[test]
    fn test_view_key_equality() {
        let key1 = ViewKey::realtime(80, 24);
        let key2 = ViewKey::realtime(80, 24);
        let key3 = ViewKey::realtime(120, 40);
        let key4 = ViewKey::historical(80, 24, 123456);
        
        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
        assert_ne!(key1, key4);
    }
    */
}
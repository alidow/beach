#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc;
    
    use crate::protocol::{
        ClientMessage, ServerMessage, ViewMode, Dimensions, SubscriptionStatus
    };
    use crate::session::multiplexer::SessionBroker;
    use crate::session::view_registry::ViewKey;
    use crate::server::terminal_state::TerminalStateTracker;
    use crate::transport::mock::MockTransport;
    
    #[ignore] // TODO: Fix async test infrastructure
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_subscription_pooling() {
        let transport = MockTransport::new();
        let tracker = Arc::new(Mutex::new(TerminalStateTracker::new(80, 24)));
        let broker = Arc::new(SessionBroker::new(transport, tracker));
        
        let (tx1, mut rx1) = mpsc::channel::<ServerMessage>(100);
        let (tx2, mut rx2) = mpsc::channel::<ServerMessage>(100);
        
        broker.add_client("client1".to_string(), tx1, false).await;
        broker.add_client("client2".to_string(), tx2, false).await;
        
        let dimensions = Dimensions { width: 80, height: 24 };
        
        let subscribe1 = ClientMessage::Subscribe {
            subscription_id: "sub1".to_string(),
            dimensions: dimensions.clone(),
            mode: ViewMode::Realtime,
            position: None,
            compression: None,
        };
        
        broker.handle_client_message("client1".to_string(), subscribe1).await.unwrap();
        
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
        
        let subscribe2 = ClientMessage::Subscribe {
            subscription_id: "sub2".to_string(),
            dimensions,
            mode: ViewMode::Realtime,
            position: None,
            compression: None,
        };
        
        broker.handle_client_message("client2".to_string(), subscribe2).await.unwrap();
        
        // First message should be SubscriptionAck
        if let Some(msg) = rx2.recv().await {
            match msg {
                ServerMessage::SubscriptionAck { status, shared_with, .. } => {
                    assert_eq!(status, SubscriptionStatus::Shared);
                    assert!(shared_with.is_some());
                    assert_eq!(shared_with.unwrap().len(), 1);
                }
                _ => panic!("Expected SubscriptionAck with shared status, got {:?}", msg),
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
        let broker = Arc::new(SessionBroker::new(transport, tracker));
        
        let (tx, mut rx) = mpsc::channel::<ServerMessage>(100);
        broker.add_client("client1".to_string(), tx, false).await;
        
        let subscribe = ClientMessage::Subscribe {
            subscription_id: "sub1".to_string(),
            dimensions: Dimensions { width: 80, height: 24 },
            mode: ViewMode::Realtime,
            position: None,
            compression: None,
        };
        
        broker.handle_client_message("client1".to_string(), subscribe).await.unwrap();
        
        rx.recv().await;
        rx.recv().await;
        
        let modify = ClientMessage::ModifySubscription {
            subscription_id: "sub1".to_string(),
            dimensions: Some(Dimensions { width: 120, height: 40 }),
            mode: None,
            position: None,
        };
        
        broker.handle_client_message("client1".to_string(), modify).await.unwrap();
        
        if let Some(msg) = rx.recv().await {
            match msg {
                ServerMessage::ViewTransition { from_mode, to_mode, .. } => {
                    assert_eq!(from_mode, ViewMode::Realtime);
                    assert_eq!(to_mode, ViewMode::Realtime);
                }
                _ => panic!("Expected ViewTransition"),
            }
        }
    }
    
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
}
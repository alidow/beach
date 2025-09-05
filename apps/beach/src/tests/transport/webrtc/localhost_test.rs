#[cfg(test)]
mod tests {
    use crate::transport::{Transport, TransportMode};
    use crate::transport::webrtc::{WebRTCTransport, config::WebRTCConfig, signaling::LocalSignalingChannel};
    use std::sync::Arc;
    use tokio::sync::Barrier;
    
    #[tokio::test]
    async fn test_full_localhost_integration() {
        // Create configs for server and client
        let server_config = WebRTCConfig::localhost(TransportMode::Server);
        let client_config = WebRTCConfig::localhost(TransportMode::Client);
        
        // Create transports
        let server_transport = WebRTCTransport::new(server_config)
            .await
            .expect("Failed to create server transport");
        let client_transport = WebRTCTransport::new(client_config)
            .await
            .expect("Failed to create client transport");
        
        // Create signaling channel pair
        let (server_signaling, client_signaling) = LocalSignalingChannel::create_pair();
        
        // Connect both transports
        let server_fut = server_transport.connect_with_local_signaling(server_signaling, true);
        let client_fut = client_transport.connect_with_local_signaling(client_signaling, false);
        
        let (server_transport, client_transport) = tokio::join!(server_fut, client_fut);
        
        let server_transport = Arc::new(tokio::sync::Mutex::new(
            server_transport.expect("Server connection failed")
        ));
        let client_transport = Arc::new(tokio::sync::Mutex::new(
            client_transport.expect("Client connection failed")
        ));
        
        // Wait for connection
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        
        // Create barrier for synchronization
        let barrier = Arc::new(Barrier::new(2));
        
        // Server task
        let server_transport_clone = server_transport.clone();
        let barrier_clone = barrier.clone();
        let server_task = tokio::spawn(async move {
            let mut transport = server_transport_clone.lock().await;
            
            // Send initial message
            transport.send(b"Server ready").await.expect("Failed to send");
            
            // Wait for client message
            barrier_clone.wait().await;
            
            // Receive client message
            let msg = transport.recv().await;
            assert!(msg.is_some());
            assert_eq!(msg.unwrap(), b"Client ready".to_vec());
            
            // Send response
            transport.send(b"Server acknowledges").await.expect("Failed to send");
        });
        
        // Client task
        let client_transport_clone = client_transport.clone();
        let barrier_clone = barrier.clone();
        let client_task = tokio::spawn(async move {
            let mut transport = client_transport_clone.lock().await;
            
            // Receive server message
            let msg = transport.recv().await;
            assert!(msg.is_some());
            assert_eq!(msg.unwrap(), b"Server ready".to_vec());
            
            // Send response
            transport.send(b"Client ready").await.expect("Failed to send");
            
            // Signal server to continue
            barrier_clone.wait().await;
            
            // Give server time to send acknowledgment
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            
            // Receive acknowledgment
            let msg = transport.recv().await;
            assert!(msg.is_some());
            assert_eq!(msg.unwrap(), b"Server acknowledges".to_vec());
        });
        
        // Wait for both tasks to complete
        let (server_result, client_result) = tokio::join!(server_task, client_task);
        server_result.expect("Server task failed");
        client_result.expect("Client task failed");
        
        // Verify connections are still valid
        let server_transport = server_transport.lock().await;
        let client_transport = client_transport.lock().await;
        assert!(server_transport.is_connected());
        assert!(client_transport.is_connected());
    }
    
    #[tokio::test]
    async fn test_large_data_transfer() {
        // Create configs for server and client
        let server_config = WebRTCConfig::localhost(TransportMode::Server);
        let client_config = WebRTCConfig::localhost(TransportMode::Client);
        
        // Create and connect transports
        let server_transport = WebRTCTransport::new(server_config)
            .await
            .expect("Failed to create server transport");
        let client_transport = WebRTCTransport::new(client_config)
            .await
            .expect("Failed to create client transport");
        
        let (server_signaling, client_signaling) = LocalSignalingChannel::create_pair();
        
        let server_fut = server_transport.connect_with_local_signaling(server_signaling, true);
        let client_fut = client_transport.connect_with_local_signaling(client_signaling, false);
        
        let (server_transport, client_transport) = tokio::join!(server_fut, client_fut);
        
        let mut server_transport = server_transport.expect("Server connection failed");
        let mut client_transport = client_transport.expect("Client connection failed");
        
        // Wait for connection
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        
        // Create large data payload (1MB)
        let large_data = vec![42u8; 1024 * 1024];
        
        // Send large data
        server_transport.send(&large_data).await.expect("Failed to send large data");
        
        // Give time for transfer
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        
        // Receive and verify
        let received = client_transport.recv().await;
        assert!(received.is_some(), "Should receive large data");
        assert_eq!(received.unwrap().len(), large_data.len(), "Data size should match");
    }
}
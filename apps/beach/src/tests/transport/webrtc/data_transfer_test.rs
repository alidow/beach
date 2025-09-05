#[cfg(test)]
mod tests {
    use crate::transport::{Transport, TransportMode};
    use crate::transport::webrtc::{WebRTCTransport, config::WebRTCConfig, signaling::LocalSignalingChannel};
    
    #[tokio::test]
    async fn test_bidirectional_data_transfer() {
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
        
        let mut server_transport = server_transport.expect("Server connection failed");
        let mut client_transport = client_transport.expect("Client connection failed");
        
        // Wait for connection to be fully established
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        
        // Test server to client communication
        let test_data = b"Hello from server!";
        server_transport.send(test_data).await.expect("Failed to send from server");
        
        // Give some time for message to arrive
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        
        let received = client_transport.recv().await;
        assert!(received.is_some(), "Client should receive data");
        assert_eq!(received.unwrap(), test_data.to_vec(), "Data should match");
        
        // Test client to server communication
        let test_data2 = b"Hello from client!";
        client_transport.send(test_data2).await.expect("Failed to send from client");
        
        // Give some time for message to arrive
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        
        let received = server_transport.recv().await;
        assert!(received.is_some(), "Server should receive data");
        assert_eq!(received.unwrap(), test_data2.to_vec(), "Data should match");
    }
    
    #[tokio::test]
    async fn test_multiple_messages() {
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
        
        // Send multiple messages
        let messages = vec![
            b"Message 1".to_vec(),
            b"Message 2".to_vec(),
            b"Message 3".to_vec(),
        ];
        
        for msg in &messages {
            server_transport.send(msg).await.expect("Failed to send message");
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
        
        // Receive and verify messages
        for expected_msg in &messages {
            let received = client_transport.recv().await;
            assert!(received.is_some(), "Should receive message");
            assert_eq!(received.unwrap(), *expected_msg, "Message should match");
        }
    }
}
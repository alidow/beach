#[cfg(test)]
mod tests {
    use crate::transport::{Transport, TransportMode};
    use crate::transport::webrtc::{WebRTCTransport, config::WebRTCConfig, signaling::LocalSignalingChannel};
    use tokio::time::{timeout, Duration};

    #[tokio::test]
    async fn test_webrtc_echo() {
        // Create local signaling channel pair
        let (server_signaling, client_signaling) = LocalSignalingChannel::create_pair();
        
        // Create configs for server and client using localhost (no STUN)
        let server_config = WebRTCConfig::localhost(TransportMode::Server);
        let client_config = WebRTCConfig::localhost(TransportMode::Client);
        
        // Create transports
        let server_transport = WebRTCTransport::new(server_config)
            .await
            .expect("Failed to create server transport");
        let client_transport = WebRTCTransport::new(client_config)
            .await
            .expect("Failed to create client transport");
        
        // Connect with server as offerer, client as answerer
        let server_fut = server_transport.connect_with_local_signaling(server_signaling, true);
        let client_fut = client_transport.connect_with_local_signaling(client_signaling, false);
        
        // Execute connection in parallel with timeout
        let (server_result, client_result) = tokio::select! {
            result = async {
                tokio::join!(server_fut, client_fut)
            } => result,
            _ = tokio::time::sleep(Duration::from_secs(5)) => {
                panic!("Connection timeout after 5 seconds");
            }
        };
        
        let mut server_transport = server_result.expect("Server connection failed");
        let mut client_transport = client_result.expect("Client connection failed");
        
        // Wait a bit for connection to stabilize
        tokio::time::sleep(Duration::from_millis(500)).await;
        
        // Verify both are connected
        assert!(server_transport.is_connected(), "Server should be connected");
        assert!(client_transport.is_connected(), "Client should be connected");
        
        // Test bidirectional data transfer
        
        // Server -> Client
        let test_data_1 = b"Hello from server".to_vec();
        server_transport.send(&test_data_1)
            .await
            .expect("Server send failed");
        
        let received_1 = timeout(
            Duration::from_secs(1),
            client_transport.recv()
        ).await
            .expect("Client receive timeout")
            .expect("Client receive failed");
        
        assert_eq!(received_1, test_data_1, "Server->Client data mismatch");
        
        // Client -> Server
        let test_data_2 = b"Hello from client".to_vec();
        client_transport.send(&test_data_2)
            .await
            .expect("Client send failed");
        
        let received_2 = timeout(
            Duration::from_secs(1),
            server_transport.recv()
        ).await
            .expect("Server receive timeout")
            .expect("Server receive failed");
        
        assert_eq!(received_2, test_data_2, "Client->Server data mismatch");
        
        // Test larger message (to verify chunking if needed)
        let large_data = vec![0xAB; 100_000]; // 100KB message
        server_transport.send(&large_data)
            .await
            .expect("Server send large message failed");
        
        let received_large = timeout(
            Duration::from_secs(2),
            client_transport.recv()
        ).await
            .expect("Client receive large message timeout")
            .expect("Client receive large message failed");
        
        assert_eq!(received_large.len(), large_data.len(), "Large message size mismatch");
        assert_eq!(received_large, large_data, "Large message data mismatch");
    }

    #[tokio::test]
    async fn test_webrtc_multiple_messages() {
        let (server_signaling, client_signaling) = LocalSignalingChannel::create_pair();
        
        let server_config = WebRTCConfig::localhost(TransportMode::Server);
        let client_config = WebRTCConfig::localhost(TransportMode::Client);
        
        let server_transport = WebRTCTransport::new(server_config)
            .await
            .expect("Failed to create server transport");
        let client_transport = WebRTCTransport::new(client_config)
            .await
            .expect("Failed to create client transport");
        
        // Connect
        let (server_transport, client_transport) = tokio::join!(
            server_transport.connect_with_local_signaling(server_signaling, true),
            client_transport.connect_with_local_signaling(client_signaling, false)
        );
        
        let mut server_transport = server_transport.expect("Server connection failed");
        let mut client_transport = client_transport.expect("Client connection failed");
        
        tokio::time::sleep(Duration::from_millis(500)).await;
        
        // Send multiple messages rapidly
        let num_messages = 10;
        
        // Send messages from server
        for i in 0..num_messages {
            let msg = format!("Message {}", i).into_bytes();
            server_transport.send(&msg)
                .await
                .expect(&format!("Failed to send message {}", i));
        }
        
        // Receive messages on client
        let mut received_messages = Vec::new();
        for _ in 0..num_messages {
            let msg = timeout(
                Duration::from_secs(1),
                client_transport.recv()
            ).await
                .expect("Receive timeout")
                .expect("Receive failed");
            received_messages.push(msg);
        }
        
        assert_eq!(received_messages.len(), num_messages, "Not all messages received");
        
        // Verify message contents
        for (i, msg) in received_messages.iter().enumerate() {
            let expected = format!("Message {}", i).into_bytes();
            assert_eq!(*msg, expected, "Message {} content mismatch", i);
        }
    }
}
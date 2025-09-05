#[cfg(test)]
mod tests {
    use crate::transport::{Transport, TransportMode};
    use crate::transport::webrtc::{WebRTCTransport, config::WebRTCConfig, signaling::LocalSignalingChannel};
    
    #[tokio::test]
    async fn test_webrtc_connection_establishment() {
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
        
        // Execute connection in parallel
        let (server_transport, client_transport) = tokio::join!(server_fut, client_fut);
        
        let server_transport = server_transport.expect("Server connection failed");
        let client_transport = client_transport.expect("Client connection failed");
        
        // Wait a bit for connection to establish
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        
        // Check that both are connected
        assert!(server_transport.is_connected(), "Server should be connected");
        assert!(client_transport.is_connected(), "Client should be connected");
    }
    
    #[tokio::test]
    async fn test_transport_modes() {
        let server_config = WebRTCConfig::localhost(TransportMode::Server);
        let client_config = WebRTCConfig::localhost(TransportMode::Client);
        
        let server_transport = WebRTCTransport::new(server_config)
            .await
            .expect("Failed to create server transport");
        let client_transport = WebRTCTransport::new(client_config)
            .await
            .expect("Failed to create client transport");
        
        assert!(matches!(server_transport.transport_mode(), TransportMode::Server));
        assert!(matches!(client_transport.transport_mode(), TransportMode::Client));
    }
}
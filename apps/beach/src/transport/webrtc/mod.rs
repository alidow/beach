use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex as AsyncMutex, RwLock as AsyncRwLock};

use webrtc::api::APIBuilder;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_candidate::RTCIceCandidate;
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::RTCPeerConnection;

use super::{Transport, TransportMode};

pub mod config;
pub mod signaling;

use config::WebRTCConfig;
use signaling::{LocalSignalingChannel, SignalingMessage};

/// WebRTC implementation of the Transport trait
pub struct WebRTCTransport {
    mode: TransportMode,
    peer_connection: Arc<RTCPeerConnection>,
    data_channel: Arc<AsyncRwLock<Option<Arc<RTCDataChannel>>>>,
    tx: Arc<AsyncMutex<mpsc::UnboundedSender<Vec<u8>>>>,
    rx: Arc<AsyncRwLock<mpsc::UnboundedReceiver<Vec<u8>>>>,
    connected: Arc<AsyncRwLock<bool>>,
    signaling_channel: Option<LocalSignalingChannel>,
}

impl WebRTCTransport {
    /// Create a new WebRTC transport
    pub async fn new(config: WebRTCConfig) -> Result<Self> {
        let mode = config.mode.clone();
        
        // Create WebRTC API
        let api = APIBuilder::new().build();
        
        // Create RTCConfiguration from our config
        let rtc_config = RTCConfiguration {
            ice_servers: config.ice_servers,
            ..Default::default()
        };
        
        // Create peer connection
        let peer_connection = Arc::new(api.new_peer_connection(rtc_config).await?);
        
        // Create channels for data flow
        let (tx_out, mut rx_out) = mpsc::unbounded_channel::<Vec<u8>>();
        let (tx_in, rx_in) = mpsc::unbounded_channel::<Vec<u8>>();
        
        let connected = Arc::new(AsyncRwLock::new(false));
        let data_channel = Arc::new(AsyncRwLock::new(None));
        
        // Setup connection state handler
        let connected_clone = connected.clone();
        peer_connection.on_ice_connection_state_change(Box::new(move |state: RTCIceConnectionState| {
            let connected = connected_clone.clone();
            Box::pin(async move {
                match state {
                    RTCIceConnectionState::Connected | RTCIceConnectionState::Completed => {
                        *connected.write().await = true;
                    }
                    RTCIceConnectionState::Failed | RTCIceConnectionState::Disconnected => {
                        *connected.write().await = false;
                    }
                    _ => {}
                }
            })
        }));
        
        // Setup peer connection state handler
        let connected_clone = connected.clone();
        peer_connection.on_peer_connection_state_change(Box::new(move |state: RTCPeerConnectionState| {
            let connected = connected_clone.clone();
            Box::pin(async move {
                match state {
                    RTCPeerConnectionState::Connected => {
                        *connected.write().await = true;
                    }
                    RTCPeerConnectionState::Failed | RTCPeerConnectionState::Disconnected => {
                        *connected.write().await = false;
                    }
                    _ => {}
                }
            })
        }));
        
        // If we're the server, create the data channel
        if matches!(mode, TransportMode::Server) {
            let dc = peer_connection.create_data_channel(&config.data_channel_label, None).await?;
            setup_data_channel(dc.clone(), tx_in.clone(), rx_out).await;
            *data_channel.write().await = Some(dc);
        } else {
            // Client mode: wait for data channel from server
            let data_channel_clone = data_channel.clone();
            let tx_in_clone = tx_in.clone();
            peer_connection.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
                let data_channel = data_channel_clone.clone();
                let tx_in = tx_in_clone.clone();
                let rx_out = mpsc::unbounded_channel::<Vec<u8>>().1; // Create dummy receiver
                
                Box::pin(async move {
                    setup_data_channel(dc.clone(), tx_in, rx_out).await;
                    *data_channel.write().await = Some(dc);
                })
            }));
        }
        
        Ok(Self {
            mode,
            peer_connection,
            data_channel,
            tx: Arc::new(AsyncMutex::new(tx_out)),
            rx: Arc::new(AsyncRwLock::new(rx_in)),
            connected,
            signaling_channel: None,
        })
    }
    
    /// Connect using a local signaling channel (for testing)
    pub async fn connect_with_local_signaling(
        mut self,
        signaling_channel: LocalSignalingChannel,
        is_offerer: bool,
    ) -> Result<Self> {
        self.signaling_channel = Some(signaling_channel);
        
        if is_offerer {
            // Create and send offer
            let offer = self.peer_connection.create_offer(None).await?;
            self.peer_connection.set_local_description(offer.clone()).await?;
            
            if let Some(ref channel) = self.signaling_channel {
                channel.send(SignalingMessage::from_offer(&offer)).await?;
            }
            
            // Wait for answer
            if let Some(ref channel) = self.signaling_channel {
                if let Some(SignalingMessage::Answer { .. }) = channel.recv().await {
                    if let Some(msg) = channel.recv().await {
                        if let Ok(answer) = msg.to_session_description() {
                            self.peer_connection.set_remote_description(answer).await?;
                        }
                    }
                }
            }
        } else {
            // Wait for offer
            if let Some(ref channel) = self.signaling_channel {
                if let Some(msg) = channel.recv().await {
                    if let Ok(offer) = msg.to_session_description() {
                        self.peer_connection.set_remote_description(offer).await?;
                        
                        // Create and send answer
                        let answer = self.peer_connection.create_answer(None).await?;
                        self.peer_connection.set_local_description(answer.clone()).await?;
                        channel.send(SignalingMessage::from_answer(&answer)).await?;
                    }
                }
            }
        }
        
        // Handle ICE candidates
        let peer_connection = self.peer_connection.clone();
        let signaling_channel = self.signaling_channel.clone();
        
        // Send our ICE candidates
        peer_connection.on_ice_candidate(Box::new(move |candidate: Option<RTCIceCandidate>| {
            let channel = signaling_channel.clone();
            Box::pin(async move {
                if let (Some(candidate), Some(channel)) = (candidate, channel) {
                    let _ = channel.send(SignalingMessage::from_ice_candidate(&candidate)).await;
                }
            })
        }));
        
        // Receive remote ICE candidates in background
        let peer_connection_clone = self.peer_connection.clone();
        if let Some(channel) = self.signaling_channel.clone() {
            tokio::spawn(async move {
                while let Some(msg) = channel.recv().await {
                    if let SignalingMessage::IceCandidate { candidate, sdp_mid, sdp_mline_index } = msg {
                        use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
                        let ice_candidate = RTCIceCandidateInit {
                            candidate,
                            sdp_mid,
                            sdp_mline_index,
                            username_fragment: None,
                        };
                        
                        let _ = peer_connection_clone.add_ice_candidate(ice_candidate).await;
                    }
                }
            });
        }
        
        Ok(self)
    }
    
    /// Close the WebRTC connection
    pub async fn close(&mut self) {
        *self.connected.write().await = false;
        let _ = self.peer_connection.close().await;
    }
}

/// Setup data channel handlers
async fn setup_data_channel(
    data_channel: Arc<RTCDataChannel>,
    tx_in: mpsc::UnboundedSender<Vec<u8>>,
    mut rx_out: mpsc::UnboundedReceiver<Vec<u8>>,
) {
    // Handle incoming messages
    data_channel.on_message(Box::new(move |msg: DataChannelMessage| {
        let tx = tx_in.clone();
        Box::pin(async move {
            let _ = tx.send(msg.data.to_vec());
        })
    }));
    
    // Handle outgoing messages
    let dc = data_channel.clone();
    tokio::spawn(async move {
        while let Some(data) = rx_out.recv().await {
            let _ = dc.send(&data.into()).await;
        }
    });
}

#[async_trait]
impl Transport for WebRTCTransport {
    async fn send(&self, data: &[u8]) -> Result<()> {
        // Send through data channel if connected
        if let Some(dc) = &*self.data_channel.read().await {
            dc.send(&data.to_vec().into()).await?;
        } else {
            // Queue message if not yet connected
            let tx = self.tx.lock().await;
            tx.send(data.to_vec())
                .map_err(|e| anyhow::anyhow!("Failed to queue data: {}", e))?;
        }
        Ok(())
    }
    
    async fn recv(&mut self) -> Option<Vec<u8>> {
        let mut rx = self.rx.write().await;
        rx.recv().await
    }
    
    fn is_connected(&self) -> bool {
        // Use try_read to avoid blocking
        self.connected.try_read()
            .map(|guard| *guard)
            .unwrap_or(false)
    }
    
    fn transport_mode(&self) -> TransportMode {
        self.mode.clone()
    }
}

impl Drop for WebRTCTransport {
    fn drop(&mut self) {
        // Clean up is handled by WebRTC library
    }
}
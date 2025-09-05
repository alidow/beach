use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::{mpsc, Mutex as AsyncMutex, RwLock as AsyncRwLock};

use webrtc::api::APIBuilder;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_candidate::{RTCIceCandidate, RTCIceCandidateInit};
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::RTCPeerConnection;

use super::{Transport, TransportMode};

pub mod config;
pub mod signaling;

use config::WebRTCConfig;
use signaling::{LocalSignalingChannel, SignalingMessage};

/// Maximum size for a single WebRTC message (64KB - overhead)
const MAX_MESSAGE_SIZE: usize = 60_000;

/// Message chunking protocol
#[derive(Debug, Clone)]
enum ChunkedMessage {
    /// Single complete message
    Single(Vec<u8>),
    /// Start of a chunked message (id, total_chunks, chunk_data)
    Start(u32, u32, Vec<u8>),
    /// Middle chunk of a message (id, chunk_index, chunk_data)
    Chunk(u32, u32, Vec<u8>),
    /// End of a chunked message (id, chunk_index, chunk_data)
    End(u32, u32, Vec<u8>),
}

impl ChunkedMessage {
    fn serialize(&self) -> Vec<u8> {
        match self {
            ChunkedMessage::Single(data) => {
                let mut result = vec![0]; // Type: Single
                result.extend_from_slice(data);
                result
            }
            ChunkedMessage::Start(id, total, data) => {
                let mut result = vec![1]; // Type: Start
                result.extend_from_slice(&id.to_be_bytes());
                result.extend_from_slice(&total.to_be_bytes());
                result.extend_from_slice(data);
                result
            }
            ChunkedMessage::Chunk(id, index, data) => {
                let mut result = vec![2]; // Type: Chunk
                result.extend_from_slice(&id.to_be_bytes());
                result.extend_from_slice(&index.to_be_bytes());
                result.extend_from_slice(data);
                result
            }
            ChunkedMessage::End(id, index, data) => {
                let mut result = vec![3]; // Type: End
                result.extend_from_slice(&id.to_be_bytes());
                result.extend_from_slice(&index.to_be_bytes());
                result.extend_from_slice(data);
                result
            }
        }
    }
    
    fn deserialize(data: &[u8]) -> Option<Self> {
        if data.is_empty() {
            return None;
        }
        
        match data[0] {
            0 => Some(ChunkedMessage::Single(data[1..].to_vec())),
            1 if data.len() > 9 => {
                let id = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
                let total = u32::from_be_bytes([data[5], data[6], data[7], data[8]]);
                Some(ChunkedMessage::Start(id, total, data[9..].to_vec()))
            }
            2 if data.len() > 9 => {
                let id = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
                let index = u32::from_be_bytes([data[5], data[6], data[7], data[8]]);
                Some(ChunkedMessage::Chunk(id, index, data[9..].to_vec()))
            }
            3 if data.len() > 9 => {
                let id = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
                let index = u32::from_be_bytes([data[5], data[6], data[7], data[8]]);
                Some(ChunkedMessage::End(id, index, data[9..].to_vec()))
            }
            _ => None,
        }
    }
}

/// Handles reassembly of chunked messages
#[derive(Debug)]
struct ChunkReassembler {
    chunks: HashMap<u32, Vec<Option<Vec<u8>>>>,
    expected_totals: HashMap<u32, u32>,
}

impl ChunkReassembler {
    fn new() -> Self {
        Self {
            chunks: HashMap::new(),
            expected_totals: HashMap::new(),
        }
    }
    
    fn add_chunk(&mut self, msg: ChunkedMessage) -> Option<Vec<u8>> {
        match msg {
            ChunkedMessage::Single(data) => Some(data),
            ChunkedMessage::Start(id, total, data) => {
                let mut chunks = vec![None; total as usize];
                chunks[0] = Some(data);
                self.chunks.insert(id, chunks);
                self.expected_totals.insert(id, total);
                None
            }
            ChunkedMessage::Chunk(id, index, data) => {
                if let Some(chunks) = self.chunks.get_mut(&id) {
                    if (index as usize) < chunks.len() {
                        chunks[index as usize] = Some(data);
                    }
                }
                None
            }
            ChunkedMessage::End(id, index, data) => {
                if let Some(chunks) = self.chunks.get_mut(&id) {
                    if (index as usize) < chunks.len() {
                        chunks[index as usize] = Some(data);
                        
                        // Check if we have all chunks
                        if chunks.iter().all(|c| c.is_some()) {
                            let mut result = Vec::new();
                            for chunk in chunks {
                                if let Some(data) = chunk {
                                    result.extend_from_slice(data);
                                }
                            }
                            self.chunks.remove(&id);
                            self.expected_totals.remove(&id);
                            return Some(result);
                        }
                    }
                }
                None
            }
        }
    }
}

/// WebRTC implementation of the Transport trait
pub struct WebRTCTransport {
    mode: TransportMode,
    peer_connection: Arc<RTCPeerConnection>,
    data_channel: Arc<AsyncRwLock<Option<Arc<RTCDataChannel>>>>,
    tx_out: Arc<AsyncMutex<mpsc::UnboundedSender<Vec<u8>>>>,
    rx_in: Arc<AsyncRwLock<mpsc::UnboundedReceiver<Vec<u8>>>>,
    connected: Arc<AsyncRwLock<bool>>,
    signaling_channel: Option<LocalSignalingChannel>,
    next_message_id: Arc<AsyncMutex<u32>>,
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
        let (tx_out, rx_out) = mpsc::unbounded_channel::<Vec<u8>>();
        let (tx_in, rx_in) = mpsc::unbounded_channel::<Vec<u8>>();
        
        let connected = Arc::new(AsyncRwLock::new(false));
        let data_channel = Arc::new(AsyncRwLock::new(None));
        
        // Setup connection state handler
        let connected_clone = connected.clone();
        peer_connection.on_ice_connection_state_change(Box::new(move |state: RTCIceConnectionState| {
            let connected = connected_clone.clone();
            Box::pin(async move {
                eprintln!("ICE connection state changed: {:?}", state);
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
                eprintln!("Peer connection state changed: {:?}", state);
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
        
        // If we're the server, create the data channel with proper configuration
        if matches!(mode, TransportMode::Server) {
            let data_channel_init = RTCDataChannelInit {
                ordered: Some(config.ordered),
                max_retransmits: config.max_retransmits,
                ..Default::default()
            };
            
            let dc = peer_connection.create_data_channel(&config.data_channel_label, Some(data_channel_init)).await?;
            
            // Setup data channel with proper channels
            let connected_for_dc = connected.clone();
            let tx_in_clone = tx_in.clone();
            let rx_out_clone = Arc::new(AsyncMutex::new(rx_out));
            
            setup_data_channel(dc.clone(), tx_in_clone, rx_out_clone, connected_for_dc).await;
            *data_channel.write().await = Some(dc);
        } else {
            // Client mode: wait for data channel from server
            let data_channel_clone = data_channel.clone();
            let tx_in_clone = tx_in.clone();
            let rx_out = Arc::new(AsyncMutex::new(rx_out));
            let connected_for_dc = connected.clone();
            
            peer_connection.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
                let data_channel = data_channel_clone.clone();
                let tx_in = tx_in_clone.clone();
                let rx_out = rx_out.clone();
                let connected = connected_for_dc.clone();
                
                Box::pin(async move {
                    eprintln!("Data channel received: {}", dc.label());
                    setup_data_channel(dc.clone(), tx_in, rx_out, connected).await;
                    *data_channel.write().await = Some(dc);
                })
            }));
        }
        
        Ok(Self {
            mode,
            peer_connection,
            data_channel,
            tx_out: Arc::new(AsyncMutex::new(tx_out)),
            rx_in: Arc::new(AsyncRwLock::new(rx_in)),
            connected,
            signaling_channel: None,
            next_message_id: Arc::new(AsyncMutex::new(0)),
        })
    }
    
    /// Connect using a local signaling channel (for testing)
    pub async fn connect_with_local_signaling(
        mut self,
        signaling_channel: LocalSignalingChannel,
        is_offerer: bool,
    ) -> Result<Self> {
        self.signaling_channel = Some(signaling_channel.clone());
        
        // Setup ICE candidate handler BEFORE creating offer/answer
        let signaling_for_ice = signaling_channel.clone();
        self.peer_connection.on_ice_candidate(Box::new(move |candidate: Option<RTCIceCandidate>| {
            let channel = signaling_for_ice.clone();
            Box::pin(async move {
                if let Some(candidate) = candidate {
                    eprintln!("Sending ICE candidate: {}", candidate.to_json().unwrap_or_default().candidate);
                    let _ = channel.send(SignalingMessage::from_ice_candidate(&candidate)).await;
                }
            })
        }));
        
        // Start ICE candidate receiver task
        let peer_connection_for_ice = self.peer_connection.clone();
        let signaling_for_recv = signaling_channel.clone();
        tokio::spawn(async move {
            while let Some(msg) = signaling_for_recv.recv().await {
                if let SignalingMessage::IceCandidate { candidate, sdp_mid, sdp_mline_index } = msg {
                    eprintln!("Received ICE candidate: {}", candidate);
                    let ice_candidate = RTCIceCandidateInit {
                        candidate,
                        sdp_mid,
                        sdp_mline_index,
                        username_fragment: None,
                    };
                    
                    if let Err(e) = peer_connection_for_ice.add_ice_candidate(ice_candidate).await {
                        eprintln!("Failed to add ICE candidate: {}", e);
                    }
                }
            }
        });
        
        // Now handle offer/answer exchange
        if is_offerer {
            // Create and send offer
            eprintln!("Creating offer...");
            let offer = self.peer_connection.create_offer(None).await?;
            self.peer_connection.set_local_description(offer.clone()).await?;
            signaling_channel.send(SignalingMessage::from_offer(&offer)).await?;
            eprintln!("Offer sent, waiting for answer...");
            
            // Wait for answer
            while let Some(msg) = signaling_channel.recv().await {
                if let SignalingMessage::Answer { .. } = msg {
                    if let Ok(answer) = msg.to_session_description() {
                        eprintln!("Received answer, setting remote description...");
                        self.peer_connection.set_remote_description(answer).await?;
                        break;
                    }
                }
            }
        } else {
            // Wait for offer
            eprintln!("Waiting for offer...");
            while let Some(msg) = signaling_channel.recv().await {
                if let SignalingMessage::Offer { .. } = msg {
                    if let Ok(offer) = msg.to_session_description() {
                        eprintln!("Received offer, setting remote description...");
                        self.peer_connection.set_remote_description(offer).await?;
                        
                        // Create and send answer
                        eprintln!("Creating answer...");
                        let answer = self.peer_connection.create_answer(None).await?;
                        self.peer_connection.set_local_description(answer.clone()).await?;
                        signaling_channel.send(SignalingMessage::from_answer(&answer)).await?;
                        eprintln!("Answer sent");
                        break;
                    }
                }
            }
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
    rx_out: Arc<AsyncMutex<mpsc::UnboundedReceiver<Vec<u8>>>>,
    connected: Arc<AsyncRwLock<bool>>,
) {
    let reassembler = Arc::new(AsyncMutex::new(ChunkReassembler::new()));
    let connected_for_open = connected.clone();
    let connected_for_close = connected.clone();
    
    // Handle data channel open event
    data_channel.on_open(Box::new(move || {
        let connected = connected_for_open.clone();
        Box::pin(async move {
            eprintln!("Data channel opened!");
            *connected.write().await = true;
        })
    }));
    
    // Handle data channel close event
    data_channel.on_close(Box::new(move || {
        let connected = connected_for_close.clone();
        Box::pin(async move {
            eprintln!("Data channel closed!");
            *connected.write().await = false;
        })
    }));
    
    // Handle incoming messages
    let reassembler_for_msg = reassembler.clone();
    data_channel.on_message(Box::new(move |msg: DataChannelMessage| {
        let tx = tx_in.clone();
        let reassembler = reassembler_for_msg.clone();
        Box::pin(async move {
            eprintln!("Received raw message: {} bytes", msg.data.len());
            
            // Deserialize and reassemble
            if let Some(chunked_msg) = ChunkedMessage::deserialize(&msg.data) {
                let mut reassembler = reassembler.lock().await;
                if let Some(complete_data) = reassembler.add_chunk(chunked_msg) {
                    eprintln!("Reassembled complete message: {} bytes", complete_data.len());
                    let _ = tx.send(complete_data);
                }
            }
        })
    }));
    
    // Handle outgoing messages with chunking
    let dc = data_channel.clone();
    let next_id = Arc::new(AsyncMutex::new(0u32));
    tokio::spawn(async move {
        let mut rx = rx_out.lock().await;
        while let Some(data) = rx.recv().await {
            eprintln!("Preparing to send message: {} bytes", data.len());
            
            if data.len() <= MAX_MESSAGE_SIZE {
                // Small message, send as single
                let msg = ChunkedMessage::Single(data);
                let serialized = msg.serialize();
                eprintln!("Sending single message: {} bytes", serialized.len());
                if let Err(e) = dc.send(&serialized.into()).await {
                    eprintln!("Failed to send data: {}", e);
                }
            } else {
                // Large message, chunk it
                let mut id = next_id.lock().await;
                let msg_id = *id;
                *id = id.wrapping_add(1);
                drop(id);
                
                let chunks: Vec<Vec<u8>> = data.chunks(MAX_MESSAGE_SIZE)
                    .map(|c| c.to_vec())
                    .collect();
                let total_chunks = chunks.len() as u32;
                
                eprintln!("Chunking large message into {} chunks", total_chunks);
                
                for (index, chunk) in chunks.into_iter().enumerate() {
                    let msg = if index == 0 {
                        ChunkedMessage::Start(msg_id, total_chunks, chunk)
                    } else if index == (total_chunks as usize - 1) {
                        ChunkedMessage::End(msg_id, index as u32, chunk)
                    } else {
                        ChunkedMessage::Chunk(msg_id, index as u32, chunk)
                    };
                    
                    let serialized = msg.serialize();
                    eprintln!("Sending chunk {} of {}: {} bytes", index + 1, total_chunks, serialized.len());
                    if let Err(e) = dc.send(&serialized.into()).await {
                        eprintln!("Failed to send chunk: {}", e);
                        break;
                    }
                    
                    // Small delay between chunks to avoid overwhelming
                    tokio::time::sleep(tokio::time::Duration::from_micros(100)).await;
                }
            }
        }
    });
}

#[async_trait]
impl Transport for WebRTCTransport {
    async fn send(&self, data: &[u8]) -> Result<()> {
        // Always queue through the channel to handle chunking
        eprintln!("Transport send: {} bytes", data.len());
        let tx = self.tx_out.lock().await;
        tx.send(data.to_vec())
            .map_err(|e| anyhow::anyhow!("Failed to queue data: {}", e))?;
        Ok(())
    }
    
    async fn recv(&mut self) -> Option<Vec<u8>> {
        let mut rx = self.rx_in.write().await;
        let result = rx.recv().await;
        if let Some(ref data) = result {
            eprintln!("Transport recv: {} bytes", data.len());
        }
        result
    }
    
    fn is_connected(&self) -> bool {
        // Use try_read to avoid blocking
        let connected = self.connected.try_read()
            .map(|guard| *guard)
            .unwrap_or(false);
        eprintln!("Transport is_connected: {}", connected);
        connected
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
use crate::debug_log::DebugLogger;
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex as AsyncMutex, RwLock as AsyncRwLock, mpsc};

use webrtc::api::APIBuilder;
use webrtc::data_channel::RTCDataChannel;
use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::ice_transport::ice_candidate::{RTCIceCandidate, RTCIceCandidateInit};
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;

use super::{ChannelPurpose, ChannelReliability, Transport, TransportChannel, TransportMode};

pub mod config;
pub mod remote_signaling;
pub mod signaling;

use config::WebRTCConfig;
use remote_signaling::RemoteSignalingChannel;
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

/// WebRTC channel implementation
pub struct WebRTCChannel {
    purpose: ChannelPurpose,
    reliability: ChannelReliability,
    label: String,
    data_channel: Arc<RTCDataChannel>,
    tx_out: Arc<AsyncMutex<mpsc::UnboundedSender<Vec<u8>>>>,
    rx_in: Arc<AsyncRwLock<mpsc::UnboundedReceiver<Vec<u8>>>>,
    is_open: Arc<AsyncRwLock<bool>>,
}

#[async_trait]
impl TransportChannel for WebRTCChannel {
    fn label(&self) -> &str {
        &self.label
    }

    fn reliability(&self) -> ChannelReliability {
        self.reliability
    }

    fn purpose(&self) -> ChannelPurpose {
        self.purpose
    }

    async fn send(&self, data: &[u8]) -> Result<()> {
        let tx = self.tx_out.lock().await;
        tx.send(data.to_vec())
            .map_err(|e| anyhow::anyhow!("Failed to queue data: {}", e))?;
        Ok(())
    }

    async fn recv(&mut self) -> Option<Vec<u8>> {
        let mut rx = self.rx_in.write().await;
        rx.recv().await
    }

    fn is_open(&self) -> bool {
        self.is_open.try_read().map(|guard| *guard).unwrap_or(false)
    }
}

/// WebRTC implementation of the Transport trait
pub struct WebRTCTransport {
    mode: TransportMode,
    peer_connection: Arc<RTCPeerConnection>,
    channels: Arc<AsyncRwLock<HashMap<ChannelPurpose, Arc<WebRTCChannel>>>>,
    data_channel: Arc<AsyncRwLock<Option<Arc<RTCDataChannel>>>>, // Legacy single channel
    tx_out: Arc<AsyncMutex<mpsc::UnboundedSender<Vec<u8>>>>,
    rx_in: Arc<AsyncRwLock<mpsc::UnboundedReceiver<Vec<u8>>>>,
    connected: Arc<AsyncRwLock<bool>>,
    signaling_channel: Option<LocalSignalingChannel>,
    next_message_id: Arc<AsyncMutex<u32>>,
    debug_logger: Option<crate::debug_log::DebugLogger>,
}

/// Macro for debug logging in WebRTC transport
macro_rules! webrtc_log {
    ($self:expr, $($arg:tt)*) => {
        if let Some(ref logger) = $self.debug_logger {
            logger.log(&format!("[WebRTC] {}", format!($($arg)*)));
        }
    };
}

impl WebRTCTransport {
    /// Create a new WebRTC transport
    pub async fn new(config: WebRTCConfig) -> Result<Self> {
        let mode = config.mode.clone();
        let debug_logger = config.debug_logger.clone();

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
        let channels = Arc::new(AsyncRwLock::new(HashMap::new()));
        let data_channel = Arc::new(AsyncRwLock::new(None));

        // Setup connection state handler
        let connected_clone = connected.clone();
        let debug_logger_ice = debug_logger.clone();
        peer_connection.on_ice_connection_state_change(Box::new(
            move |state: RTCIceConnectionState| {
                let connected = connected_clone.clone();
                let logger = debug_logger_ice.clone();
                Box::pin(async move {
                    // Debug logging to file
                    if let Some(ref logger) = logger {
                        logger.log(&format!(
                            "[WebRTC] ICE connection state changed: {:?}",
                            state
                        ));
                    }
                    match state {
                        RTCIceConnectionState::Connected => {
                            if let Some(ref logger) = logger {
                                logger.log("[WebRTC] ICE Connected - P2P connection established!");
                            }
                            *connected.write().await = true;
                        }
                        RTCIceConnectionState::Completed => {
                            if let Some(ref logger) = logger {
                                logger.log("[WebRTC] ICE Completed - All candidates gathered");
                            }
                            *connected.write().await = true;
                        }
                        RTCIceConnectionState::Failed => {
                            if let Some(ref logger) = logger {
                                logger.log(
                                    "[WebRTC] ICE Failed - Connection could not be established",
                                );
                            }
                            *connected.write().await = false;
                        }
                        RTCIceConnectionState::Disconnected => {
                            if let Some(ref logger) = logger {
                                logger.log("[WebRTC] ICE Disconnected - Connection lost");
                            }
                            *connected.write().await = false;
                        }
                        RTCIceConnectionState::Checking => {
                            if let Some(ref logger) = logger {
                                logger.log(
                                    "[WebRTC] ICE Checking - Validating connection candidates",
                                );
                            }
                        }
                        _ => {}
                    }
                })
            },
        ));

        // Setup peer connection state handler
        let connected_clone = connected.clone();
        let debug_logger_pc = debug_logger.clone();
        peer_connection.on_peer_connection_state_change(Box::new(move |state: RTCPeerConnectionState| {
            let connected = connected_clone.clone();
            let logger = debug_logger_pc.clone();
            Box::pin(async move {
                if let Some(ref logger) = logger {
                    logger.log(&format!("[WebRTC] Peer connection state changed: {:?}", state));
                }
                match state {
                    RTCPeerConnectionState::Connected => {
                        if let Some(ref logger) = logger {
                            logger.log("[WebRTC] Peer Connected - Data channels ready");
                        }
                        *connected.write().await = true;
                    }
                    RTCPeerConnectionState::Failed => {
                        if let Some(ref logger) = logger {
                            logger.log("[WebRTC] Peer Failed - Connection cannot be established or has failed");
                        }
                        *connected.write().await = false;
                    }
                    RTCPeerConnectionState::Disconnected => {
                        if let Some(ref logger) = logger {
                            logger.log("[WebRTC] Peer Disconnected - Connection terminated");
                        }
                        *connected.write().await = false;
                    }
                    RTCPeerConnectionState::Connecting => {
                        if let Some(ref logger) = logger {
                            logger.log("[WebRTC] Peer Connecting - Establishing connection");
                        }
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

            let dc = peer_connection
                .create_data_channel(&config.data_channel_label, Some(data_channel_init))
                .await?;

            // Setup data channel with proper channels
            let connected_for_dc = connected.clone();
            let tx_in_clone = tx_in.clone();
            let rx_out_clone = Arc::new(AsyncMutex::new(rx_out));
            let debug_logger_dc = debug_logger.clone();

            setup_data_channel(
                dc.clone(),
                tx_in_clone,
                rx_out_clone,
                connected_for_dc,
                debug_logger_dc,
            )
            .await;
            *data_channel.write().await = Some(dc);
        } else {
            // Client mode: wait for data channel from server
            let data_channel_clone = data_channel.clone();
            let tx_in_clone = tx_in.clone();
            let rx_out = Arc::new(AsyncMutex::new(rx_out));
            let connected_for_dc = connected.clone();
            let debug_logger_client = debug_logger.clone();

            peer_connection.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
                let data_channel = data_channel_clone.clone();
                let tx_in = tx_in_clone.clone();
                let rx_out = rx_out.clone();
                let connected = connected_for_dc.clone();
                let logger = debug_logger_client.clone();

                Box::pin(async move {
                    if let Some(ref logger) = logger {
                        logger.log(&format!("[WebRTC] Data channel received: {}", dc.label()));
                    }
                    setup_data_channel(dc.clone(), tx_in, rx_out, connected, logger).await;
                    *data_channel.write().await = Some(dc);
                })
            }));
        }

        Ok(Self {
            mode,
            peer_connection,
            channels,
            data_channel,
            tx_out: Arc::new(AsyncMutex::new(tx_out)),
            rx_in: Arc::new(AsyncRwLock::new(rx_in)),
            connected,
            signaling_channel: None,
            next_message_id: Arc::new(AsyncMutex::new(0)),
            debug_logger,
        })
    }

    /// Create a channel with the given purpose
    async fn create_channel_internal(&self, purpose: ChannelPurpose) -> Result<Arc<WebRTCChannel>> {
        let label = purpose.label();
        let reliability = purpose.default_reliability();

        // Configure data channel based on reliability
        let init = match reliability {
            ChannelReliability::Reliable => RTCDataChannelInit {
                ordered: Some(true),
                ..Default::default()
            },
            ChannelReliability::Unreliable {
                max_retransmits, ..
            } => RTCDataChannelInit {
                ordered: Some(false),
                max_retransmits,
                ..Default::default()
            },
        };

        // Create the WebRTC data channel
        let rtc_channel = self
            .peer_connection
            .create_data_channel(&label, Some(init))
            .await?;

        // Create channels for this specific channel
        let (tx_out, rx_out) = mpsc::unbounded_channel::<Vec<u8>>();
        let (tx_in, rx_in) = mpsc::unbounded_channel::<Vec<u8>>();
        let is_open = Arc::new(AsyncRwLock::new(false));

        // Setup handlers for this channel
        let is_open_clone = is_open.clone();
        let label_clone = label.clone();
        rtc_channel.on_open(Box::new(move || {
            let is_open = is_open_clone.clone();
            let _label = label_clone.clone();
            Box::pin(async move {
                // // eprintln!("Channel {} opened!", label);
                *is_open.write().await = true;
            })
        }));

        let is_open_clone = is_open.clone();
        let label_clone = label.clone();
        rtc_channel.on_close(Box::new(move || {
            let is_open = is_open_clone.clone();
            let _label = label_clone.clone();
            Box::pin(async move {
                // // eprintln!("Channel {} closed!", label);
                *is_open.write().await = false;
            })
        }));

        // Setup message handling with chunking support
        let reassembler = Arc::new(AsyncMutex::new(ChunkReassembler::new()));
        let tx_in_clone = tx_in.clone();
        rtc_channel.on_message(Box::new(move |msg: DataChannelMessage| {
            let tx = tx_in_clone.clone();
            let reassembler = reassembler.clone();
            Box::pin(async move {
                if let Some(chunked_msg) = ChunkedMessage::deserialize(&msg.data) {
                    let mut reassembler = reassembler.lock().await;
                    if let Some(complete_data) = reassembler.add_chunk(chunked_msg) {
                        let _ = tx.send(complete_data);
                    }
                }
            })
        }));

        // Setup outgoing message handler
        let dc = rtc_channel.clone();
        let next_id = self.next_message_id.clone();
        tokio::spawn(async move {
            let mut rx = rx_out;
            while let Some(data) = rx.recv().await {
                if data.len() <= MAX_MESSAGE_SIZE {
                    let msg = ChunkedMessage::Single(data);
                    let serialized = msg.serialize();
                    if let Err(_e) = dc.send(&serialized.into()).await {
                        // // eprintln!("Failed to send data: {}", e);
                    }
                } else {
                    // Handle chunking for large messages
                    let mut id = next_id.lock().await;
                    let msg_id = *id;
                    *id = id.wrapping_add(1);
                    drop(id);

                    let chunks: Vec<Vec<u8>> =
                        data.chunks(MAX_MESSAGE_SIZE).map(|c| c.to_vec()).collect();
                    let total_chunks = chunks.len() as u32;

                    for (index, chunk) in chunks.into_iter().enumerate() {
                        let msg = if index == 0 {
                            ChunkedMessage::Start(msg_id, total_chunks, chunk)
                        } else if index == (total_chunks as usize - 1) {
                            ChunkedMessage::End(msg_id, index as u32, chunk)
                        } else {
                            ChunkedMessage::Chunk(msg_id, index as u32, chunk)
                        };

                        let serialized = msg.serialize();
                        if let Err(_e) = dc.send(&serialized.into()).await {
                            // // eprintln!("Failed to send chunk: {}", e);
                            break;
                        }

                        tokio::time::sleep(tokio::time::Duration::from_micros(100)).await;
                    }
                }
            }
        });

        // Create and return the WebRTC channel
        let channel = Arc::new(WebRTCChannel {
            purpose,
            reliability,
            label,
            data_channel: rtc_channel,
            tx_out: Arc::new(AsyncMutex::new(tx_out)),
            rx_in: Arc::new(AsyncRwLock::new(rx_in)),
            is_open,
        });

        Ok(channel)
    }

    /// Connect using remote signaling via beach-road (non-consuming version)
    pub async fn initiate_remote_connection(
        &self,
        signaling: Arc<RemoteSignalingChannel>,
        is_offerer: bool,
    ) -> Result<()> {
        // Setup ICE candidate handler
        let signaling_for_ice = signaling.clone();
        let debug_logger_ice = self.debug_logger.clone();
        self.peer_connection.on_ice_candidate(Box::new(
            move |candidate: Option<RTCIceCandidate>| {
                let signaling = signaling_for_ice.clone();
                let logger = debug_logger_ice.clone();
                Box::pin(async move {
                    if let Some(candidate) = candidate {
                        if let Some(ref logger) = logger {
                            logger.log(&format!(
                                "[WebRTC] Sending ICE candidate: {}",
                                candidate.to_json().unwrap_or_default().candidate
                            ));
                        }
                        if let Err(e) = signaling.send_ice_candidate(candidate).await {
                            if let Some(ref logger) = logger {
                                logger
                                    .log(&format!("[WebRTC] Failed to send ICE candidate: {}", e));
                            }
                        }
                    } else {
                        if let Some(ref logger) = logger {
                            logger.log("[WebRTC] ICE gathering complete");
                        }
                    }
                })
            },
        ));

        // Handle offer/answer exchange (MUST happen before starting ICE processor)
        if is_offerer {
            webrtc_log!(self, "Creating offer (server role)");

            // Create and send offer
            let offer = self.peer_connection.create_offer(None).await?;
            self.peer_connection
                .set_local_description(offer.clone())
                .await?;
            signaling.send_offer(offer.sdp).await?;

            // Wait for answer
            let answer = signaling.wait_for_answer().await?;
            self.peer_connection.set_remote_description(answer).await?;

            webrtc_log!(self, "Answer received and set");
        } else {
            webrtc_log!(self, "Waiting for offer (client role)");

            // Wait for offer
            let offer = signaling.wait_for_offer().await?;
            self.peer_connection.set_remote_description(offer).await?;

            // Create and send answer
            let answer = self.peer_connection.create_answer(None).await?;
            self.peer_connection
                .set_local_description(answer.clone())
                .await?;
            signaling.send_answer(answer.sdp).await?;

            webrtc_log!(self, "Answer sent");
        }

        // Start ICE candidate processor AFTER offer/answer exchange
        let pc_for_ice = self.peer_connection.clone();
        let signaling_for_recv = signaling.clone();
        let debug_logger_ice_proc = self.debug_logger.clone();
        tokio::spawn(async move {
            if let Some(ref logger) = debug_logger_ice_proc {
                logger.log("[WebRTC] Starting ICE candidate processor");
            }
            if let Err(e) = signaling_for_recv.process_ice_candidates(&pc_for_ice).await {
                if let Some(ref logger) = debug_logger_ice_proc {
                    logger.log(&format!("[WebRTC] Error processing ICE candidates: {}", e));
                }
            }
        });

        // Wait for connection to be established with periodic status checks
        let start = std::time::Instant::now();
        let mut last_log = std::time::Instant::now();
        while !self.is_connected() {
            if start.elapsed() > std::time::Duration::from_secs(30) {
                webrtc_log!(self, "WebRTC connection timeout after 30 seconds");
                return Err(anyhow::anyhow!(
                    "WebRTC connection timeout - check network and firewall settings"
                ));
            }

            // Log status every 5 seconds
            if last_log.elapsed() > std::time::Duration::from_secs(5) {
                webrtc_log!(
                    self,
                    "Still waiting for WebRTC connection... ({}s elapsed)",
                    start.elapsed().as_secs()
                );
                last_log = std::time::Instant::now();
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        webrtc_log!(
            self,
            "P2P connection established after {}ms!",
            start.elapsed().as_millis()
        );

        Ok(())
    }

    /// Connect using remote signaling via beach-road (ownership version)
    pub async fn connect_with_remote_signaling(
        self,
        signaling: Arc<RemoteSignalingChannel>,
        is_offerer: bool,
    ) -> Result<Self> {
        // Setup ICE candidate handler
        let signaling_for_ice = signaling.clone();
        let debug_logger_ice = self.debug_logger.clone();
        self.peer_connection.on_ice_candidate(Box::new(
            move |candidate: Option<RTCIceCandidate>| {
                let signaling = signaling_for_ice.clone();
                let logger = debug_logger_ice.clone();
                Box::pin(async move {
                    if let Some(candidate) = candidate {
                        if let Some(ref logger) = logger {
                            logger.log(&format!(
                                "[WebRTC] Sending ICE candidate: {}",
                                candidate.to_json().unwrap_or_default().candidate
                            ));
                        }
                        if let Err(e) = signaling.send_ice_candidate(candidate).await {
                            if let Some(ref logger) = logger {
                                logger
                                    .log(&format!("[WebRTC] Failed to send ICE candidate: {}", e));
                            }
                        }
                    } else {
                        if let Some(ref logger) = logger {
                            logger.log("[WebRTC] ICE gathering complete");
                        }
                    }
                })
            },
        ));

        // Handle offer/answer exchange (MUST happen before starting ICE processor)
        if is_offerer {
            webrtc_log!(self, "Creating offer (server role)");

            // Create and send offer
            let offer = self.peer_connection.create_offer(None).await?;
            self.peer_connection
                .set_local_description(offer.clone())
                .await?;
            signaling.send_offer(offer.sdp).await?;

            // Wait for answer
            let answer = signaling.wait_for_answer().await?;
            self.peer_connection.set_remote_description(answer).await?;

            webrtc_log!(self, "Answer received and set");
        } else {
            webrtc_log!(self, "Waiting for offer (client role)");

            // Wait for offer
            let offer = signaling.wait_for_offer().await?;
            self.peer_connection.set_remote_description(offer).await?;

            // Create and send answer
            let answer = self.peer_connection.create_answer(None).await?;
            self.peer_connection
                .set_local_description(answer.clone())
                .await?;
            signaling.send_answer(answer.sdp).await?;

            webrtc_log!(self, "Answer sent");
        }

        // Start ICE candidate processor AFTER offer/answer exchange
        let pc_for_ice = self.peer_connection.clone();
        let signaling_for_recv = signaling.clone();
        let debug_logger_ice_proc = self.debug_logger.clone();
        tokio::spawn(async move {
            if let Some(ref logger) = debug_logger_ice_proc {
                logger.log("[WebRTC] Starting ICE candidate processor");
            }
            if let Err(e) = signaling_for_recv.process_ice_candidates(&pc_for_ice).await {
                if let Some(ref logger) = debug_logger_ice_proc {
                    logger.log(&format!("[WebRTC] Error processing ICE candidates: {}", e));
                }
            }
        });

        // Wait for connection to be established with periodic status checks
        let start = std::time::Instant::now();
        let mut last_log = std::time::Instant::now();
        while !self.is_connected() {
            if start.elapsed() > std::time::Duration::from_secs(30) {
                webrtc_log!(self, "WebRTC connection timeout after 30 seconds");
                return Err(anyhow::anyhow!(
                    "WebRTC connection timeout - check network and firewall settings"
                ));
            }

            // Log status every 5 seconds
            if last_log.elapsed() > std::time::Duration::from_secs(5) {
                webrtc_log!(
                    self,
                    "Still waiting for WebRTC connection... ({}s elapsed)",
                    start.elapsed().as_secs()
                );
                last_log = std::time::Instant::now();
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        webrtc_log!(
            self,
            "P2P connection established after {}ms!",
            start.elapsed().as_millis()
        );

        Ok(self)
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
        self.peer_connection.on_ice_candidate(Box::new(
            move |candidate: Option<RTCIceCandidate>| {
                let channel = signaling_for_ice.clone();
                Box::pin(async move {
                    if let Some(candidate) = candidate {
                        // // eprintln!("Sending ICE candidate: {}", candidate.to_json().unwrap_or_default().candidate);
                        let _ = channel
                            .send(SignalingMessage::from_ice_candidate(&candidate))
                            .await;
                    }
                })
            },
        ));

        // Start ICE candidate receiver task
        let peer_connection_for_ice = self.peer_connection.clone();
        let signaling_for_recv = signaling_channel.clone();
        tokio::spawn(async move {
            while let Some(msg) = signaling_for_recv.recv().await {
                if let SignalingMessage::IceCandidate {
                    candidate,
                    sdp_mid,
                    sdp_mline_index,
                } = msg
                {
                    // // eprintln!("Received ICE candidate: {}", candidate);
                    let ice_candidate = RTCIceCandidateInit {
                        candidate,
                        sdp_mid,
                        sdp_mline_index,
                        username_fragment: None,
                    };

                    if let Err(_e) = peer_connection_for_ice
                        .add_ice_candidate(ice_candidate)
                        .await
                    {
                        // // eprintln!("Failed to add ICE candidate: {}", e);
                    }
                }
            }
        });

        // Now handle offer/answer exchange
        if is_offerer {
            // Create and send offer
            webrtc_log!(self, "Creating offer...");
            let offer = self.peer_connection.create_offer(None).await?;
            self.peer_connection
                .set_local_description(offer.clone())
                .await?;
            signaling_channel
                .send(SignalingMessage::from_offer(&offer))
                .await?;
            webrtc_log!(self, "Offer sent, waiting for answer...");

            // Wait for answer
            while let Some(msg) = signaling_channel.recv().await {
                if let SignalingMessage::Answer { .. } = msg {
                    if let Ok(answer) = msg.to_session_description() {
                        webrtc_log!(self, "Received answer, setting remote description...");
                        self.peer_connection.set_remote_description(answer).await?;
                        break;
                    }
                }
            }
        } else {
            // Wait for offer
            webrtc_log!(self, "Waiting for offer...");
            while let Some(msg) = signaling_channel.recv().await {
                if let SignalingMessage::Offer { .. } = msg {
                    if let Ok(offer) = msg.to_session_description() {
                        webrtc_log!(self, "Received offer, setting remote description...");
                        self.peer_connection.set_remote_description(offer).await?;

                        // Create and send answer
                        webrtc_log!(self, "Creating answer...");
                        let answer = self.peer_connection.create_answer(None).await?;
                        self.peer_connection
                            .set_local_description(answer.clone())
                            .await?;
                        signaling_channel
                            .send(SignalingMessage::from_answer(&answer))
                            .await?;
                        webrtc_log!(self, "Answer sent");
                        break;
                    }
                }
            }
        }

        Ok(self)
    }

    /// Take ownership of the incoming receiver for dedicated routing
    /// This allows a router task to consume messages without holding transport mutex
    pub async fn take_incoming(&self) -> Option<mpsc::UnboundedReceiver<Vec<u8>>> {
        let mut rx_guard = self.rx_in.write().await;
        // Take the receiver out, replacing with a dummy one
        let (_, dummy_rx) = mpsc::unbounded_channel();
        let actual_rx = std::mem::replace(&mut *rx_guard, dummy_rx);
        // Return the actual receiver
        Some(actual_rx)
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
    debug_logger: Option<DebugLogger>,
) {
    let reassembler = Arc::new(AsyncMutex::new(ChunkReassembler::new()));
    let connected_for_open = connected.clone();
    let connected_for_close = connected.clone();

    // Handle data channel open event
    let debug_logger_open = debug_logger.clone();
    let label_open = "beach".to_string(); // Use hardcoded label to avoid lifetime issues
    data_channel.on_open(Box::new(move || {
        let connected = connected_for_open.clone();
        let logger = debug_logger_open.clone();
        let label = label_open.clone();
        Box::pin(async move {
            if let Some(ref logger) = logger {
                logger.log(&format!(
                    "[WebRTC] Data channel '{}' opened! Connection established.",
                    label
                ));
            }
            *connected.write().await = true;
        })
    }));

    // Handle data channel close event
    let debug_logger_close = debug_logger.clone();
    let label_close = "beach".to_string(); // Use hardcoded label to avoid lifetime issues
    data_channel.on_close(Box::new(move || {
        let connected = connected_for_close.clone();
        let logger = debug_logger_close.clone();
        let label = label_close.clone();
        Box::pin(async move {
            if let Some(ref logger) = logger {
                logger.log(&format!("[WebRTC] Data channel '{}' closed!", label));
            }
            *connected.write().await = false;
        })
    }));

    // Handle incoming messages
    let reassembler_for_msg = reassembler.clone();
    data_channel.on_message(Box::new(move |msg: DataChannelMessage| {
        let tx = tx_in.clone();
        let reassembler = reassembler_for_msg.clone();
        Box::pin(async move {
            if std::env::var("BEACH_VERBOSE").is_ok() {
                // // eprintln!("📨 [WebRTC DataChannel] Received raw message: {} bytes", msg.data.len());
            }

            // Deserialize and reassemble
            if let Some(chunked_msg) = ChunkedMessage::deserialize(&msg.data) {
                let mut reassembler = reassembler.lock().await;
                if let Some(complete_data) = reassembler.add_chunk(chunked_msg) {
                    if std::env::var("BEACH_VERBOSE").is_ok() {
                        // // eprintln!("✅ [WebRTC DataChannel] Reassembled complete message: {} bytes", complete_data.len());
                    }
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
            if std::env::var("BEACH_VERBOSE").is_ok() {
                // // eprintln!("🚀 [WebRTC DataChannel] Preparing to send message: {} bytes", data.len());
            }

            if data.len() <= MAX_MESSAGE_SIZE {
                // Small message, send as single
                let msg = ChunkedMessage::Single(data);
                let serialized = msg.serialize();
                if std::env::var("BEACH_VERBOSE").is_ok() {
                    // // eprintln!("📤 [WebRTC DataChannel] Sending single message: {} bytes", serialized.len());
                }
                if let Err(_e) = dc.send(&serialized.into()).await {
                    // // eprintln!("Failed to send data: {}", e);
                }
            } else {
                // Large message, chunk it
                let mut id = next_id.lock().await;
                let msg_id = *id;
                *id = id.wrapping_add(1);
                drop(id);

                let chunks: Vec<Vec<u8>> =
                    data.chunks(MAX_MESSAGE_SIZE).map(|c| c.to_vec()).collect();
                let total_chunks = chunks.len() as u32;

                // // eprintln!("Chunking large message into {} chunks", total_chunks);

                for (index, chunk) in chunks.into_iter().enumerate() {
                    let msg = if index == 0 {
                        ChunkedMessage::Start(msg_id, total_chunks, chunk)
                    } else if index == (total_chunks as usize - 1) {
                        ChunkedMessage::End(msg_id, index as u32, chunk)
                    } else {
                        ChunkedMessage::Chunk(msg_id, index as u32, chunk)
                    };

                    let serialized = msg.serialize();
                    // // eprintln!("Sending chunk {} of {}: {} bytes", index + 1, total_chunks, serialized.len());
                    if let Err(_e) = dc.send(&serialized.into()).await {
                        // // eprintln!("Failed to send chunk: {}", e);
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
    async fn channel(&self, purpose: ChannelPurpose) -> Result<Arc<dyn TransportChannel>> {
        // Check if channel already exists
        {
            let channels = self.channels.read().await;
            if let Some(channel) = channels.get(&purpose) {
                return Ok(channel.clone() as Arc<dyn TransportChannel>);
            }
        }

        // Create new channel if not exists
        let channel = self.create_channel_internal(purpose).await?;

        // Store the channel
        let mut channels = self.channels.write().await;
        channels.insert(purpose, channel.clone());

        Ok(channel as Arc<dyn TransportChannel>)
    }

    fn channels(&self) -> Vec<ChannelPurpose> {
        // Use try_read to avoid blocking
        self.channels
            .try_read()
            .map(|guard| guard.keys().copied().collect())
            .unwrap_or_else(|_| vec![])
    }

    fn supports_multi_channel(&self) -> bool {
        true
    }

    async fn send(&self, data: &[u8]) -> Result<()> {
        // For backward compatibility, use single channel if available,
        // otherwise create control channel
        let dc = self.data_channel.read().await;
        if dc.is_some() {
            // Use legacy single channel
            webrtc_log!(self, "Transport send (legacy): {} bytes", data.len());
            let tx = self.tx_out.lock().await;
            tx.send(data.to_vec())
                .map_err(|e| anyhow::anyhow!("Failed to queue data: {}", e))?;
            Ok(())
        } else {
            // Use control channel
            webrtc_log!(
                self,
                "Transport send (control channel): {} bytes",
                data.len()
            );
            let channel = self.channel(ChannelPurpose::Control).await?;
            channel.send(data).await
        }
    }

    async fn recv(&mut self) -> Option<Vec<u8>> {
        // Prefer legacy single channel if present
        let dc = self.data_channel.read().await;
        if dc.is_some() {
            let mut rx = self.rx_in.write().await;
            let result = rx.recv().await;
            if let Some(ref data) = result {
                webrtc_log!(self, "Transport recv (legacy): {} bytes", data.len());
            }
            return result;
        }

        // Fall back to purpose-specific channels (Control first, then Output)
        let channels = self.channels.read().await;
        // Clone Arcs to drop the map lock before awaiting
        let ctrl = channels.get(&ChannelPurpose::Control).cloned();
        let outp = channels.get(&ChannelPurpose::Output).cloned();
        drop(channels);

        if let Some(ch) = ctrl {
            // Access channel's rx_in directly (same module)
            let mut rx = ch.rx_in.write().await;
            if let Some(data) = rx.recv().await {
                webrtc_log!(self, "Transport recv (control): {} bytes", data.len());
                return Some(data);
            }
        }
        if let Some(ch) = outp {
            let mut rx = ch.rx_in.write().await;
            if let Some(data) = rx.recv().await {
                webrtc_log!(self, "Transport recv (output): {} bytes", data.len());
                return Some(data);
            }
        }
        webrtc_log!(
            self,
            "Transport recv: no data available on legacy/control/output channels"
        );
        None
    }

    fn is_connected(&self) -> bool {
        // Use try_read to avoid blocking
        let connected = self
            .connected
            .try_read()
            .map(|guard| *guard)
            .unwrap_or(false);
        webrtc_log!(self, "Transport is_connected: {}", connected);
        connected
    }

    fn transport_mode(&self) -> TransportMode {
        self.mode.clone()
    }

    async fn initiate_webrtc_with_signaling(
        &self,
        signaling: Arc<dyn std::any::Any + Send + Sync>,
        is_offerer: bool,
    ) -> Result<()> {
        // Try to downcast to Arc<RemoteSignalingChannel>
        let any_arc = signaling.clone();

        // First try to get it as an Arc<RemoteSignalingChannel>
        if let Ok(remote_signaling) = any_arc.downcast::<RemoteSignalingChannel>() {
            webrtc_log!(
                self,
                "Initiating WebRTC connection as {}",
                if is_offerer { "offerer" } else { "answerer" }
            );

            // Call the non-consuming version
            self.initiate_remote_connection(remote_signaling, is_offerer)
                .await
        } else {
            Err(anyhow::anyhow!(
                "Invalid signaling channel type - expected RemoteSignalingChannel"
            ))
        }
    }

    fn is_webrtc(&self) -> bool {
        true
    }
}

impl Drop for WebRTCTransport {
    fn drop(&mut self) {
        // Clean up is handled by WebRTC library
    }
}

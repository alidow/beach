use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use webrtc::ice_transport::ice_candidate::RTCIceCandidate;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

use crate::protocol::signaling::{
    ClientMessage, TransportSignal, WebRTCSignal,
};
use crate::session::signaling_transport::SignalingTransport;
use crate::transport::websocket::WebSocketTransport;

/// Remote signaling channel that uses beach-road for WebRTC signaling
pub struct RemoteSignalingChannel {
    /// WebSocket signaling transport to beach-road
    signaling: Arc<SignalingTransport<WebSocketTransport>>,
    /// Our peer ID (kept for future use)
    _peer_id: String,
    /// Remote peer ID (set after receiving peer info)
    remote_peer_id: Arc<RwLock<Option<String>>>,
    /// Channel for receiving signals from beach-road
    signal_rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<TransportSignal>>>,
    /// Channel for sending signals to router
    signal_tx: mpsc::UnboundedSender<TransportSignal>,
}

impl RemoteSignalingChannel {
    /// Create a new remote signaling channel
    pub fn new(
        signaling: Arc<SignalingTransport<WebSocketTransport>>,
        peer_id: String,
    ) -> Self {
        let (signal_tx, signal_rx) = mpsc::unbounded_channel();
        
        Self {
            signaling,
            _peer_id: peer_id,
            remote_peer_id: Arc::new(RwLock::new(None)),
            signal_rx: Arc::new(tokio::sync::Mutex::new(signal_rx)),
            signal_tx,
        }
    }
    
    /// Set the remote peer ID (after receiving from JoinSuccess or PeerJoined)
    pub async fn set_remote_peer(&self, peer_id: String) {
        *self.remote_peer_id.write().await = Some(peer_id);
    }
    
    /// Process incoming signal from beach-road
    pub async fn handle_signal(&self, signal: serde_json::Value) -> Result<()> {
        // Try to parse as TransportSignal
        if let Ok(transport_signal) = TransportSignal::from_value(&signal) {
            if std::env::var("BEACH_VERBOSE").is_ok() {
                // // eprintln!("📡 [WebRTC] Received signal: {:?}", transport_signal);
            }
            self.signal_tx.send(transport_signal)?;
        }
        Ok(())
    }
    
    /// Send SDP offer to remote peer
    pub async fn send_offer(&self, sdp: String) -> Result<()> {
        if std::env::var("BEACH_VERBOSE").is_ok() {
            // // eprintln!("📡 [WebRTC] Sending SDP offer via beach-road");
        }
        
        let signal = WebRTCSignal::Offer { sdp }.to_transport_signal();
        self.send_signal(signal).await
    }
    
    /// Send SDP answer to remote peer
    pub async fn send_answer(&self, sdp: String) -> Result<()> {
        if std::env::var("BEACH_VERBOSE").is_ok() {
            // // eprintln!("📡 [WebRTC] Sending SDP answer via beach-road");
        }
        
        let signal = WebRTCSignal::Answer { sdp }.to_transport_signal();
        self.send_signal(signal).await
    }
    
    /// Send ICE candidate to remote peer
    pub async fn send_ice_candidate(&self, candidate: RTCIceCandidate) -> Result<()> {
        let candidate_json = candidate.to_json()?;
        
        if std::env::var("BEACH_VERBOSE").is_ok() {
            // // eprintln!("📡 [WebRTC] Sending ICE candidate via beach-road: {}", 
            //     candidate_json.candidate);
        }
        
        let signal = WebRTCSignal::IceCandidate {
            candidate: candidate_json.candidate,
            sdp_mid: candidate_json.sdp_mid,
            sdp_mline_index: candidate_json.sdp_mline_index.map(|i| i as u32),
        }.to_transport_signal();
        
        self.send_signal(signal).await
    }
    
    /// Wait for SDP offer from remote peer
    pub async fn wait_for_offer(&self) -> Result<RTCSessionDescription> {
        if std::env::var("BEACH_VERBOSE").is_ok() {
            // // eprintln!("📡 [WebRTC] Waiting for SDP offer from remote peer");
        }
        
        let mut rx = self.signal_rx.lock().await;
        while let Some(signal) = rx.recv().await {
            if let TransportSignal::WebRTC { signal: WebRTCSignal::Offer { sdp } } = signal {
                if std::env::var("BEACH_VERBOSE").is_ok() {
                    // // eprintln!("📡 [WebRTC] Received SDP offer");
                }
                return Ok(RTCSessionDescription::offer(sdp)?);
            }
        }
        
        Err(anyhow::anyhow!("Failed to receive offer"))
    }
    
    /// Wait for SDP answer from remote peer
    pub async fn wait_for_answer(&self) -> Result<RTCSessionDescription> {
        if std::env::var("BEACH_VERBOSE").is_ok() {
            // // eprintln!("📡 [WebRTC] Waiting for SDP answer from remote peer");
        }
        
        let mut rx = self.signal_rx.lock().await;
        while let Some(signal) = rx.recv().await {
            if let TransportSignal::WebRTC { signal: WebRTCSignal::Answer { sdp } } = signal {
                if std::env::var("BEACH_VERBOSE").is_ok() {
                    // // eprintln!("📡 [WebRTC] Received SDP answer");
                }
                return Ok(RTCSessionDescription::answer(sdp)?);
            }
        }
        
        Err(anyhow::anyhow!("Failed to receive answer"))
    }
    
    /// Process ICE candidates (should be called in a loop)
    pub async fn process_ice_candidates(
        &self,
        peer_connection: &webrtc::peer_connection::RTCPeerConnection,
    ) -> Result<()> {
        let mut rx = self.signal_rx.lock().await;
        
        while let Some(signal) = rx.recv().await {
            if let TransportSignal::WebRTC { 
                signal: WebRTCSignal::IceCandidate { 
                    candidate, 
                    sdp_mid, 
                    sdp_mline_index 
                } 
            } = signal {
                if std::env::var("BEACH_VERBOSE").is_ok() {
                    // // eprintln!("📡 [WebRTC] Received ICE candidate: {}", candidate);
                }
                
                let ice_candidate = webrtc::ice_transport::ice_candidate::RTCIceCandidateInit {
                    candidate,
                    sdp_mid,
                    sdp_mline_index: sdp_mline_index.map(|i| i as u16),
                    username_fragment: None,
                };
                
                peer_connection.add_ice_candidate(ice_candidate).await?;
            }
        }
        
        Ok(())
    }
    
    /// Send a signal to the remote peer via beach-road
    async fn send_signal(&self, signal: TransportSignal) -> Result<()> {
        if let Some(to_peer) = self.remote_peer_id.read().await.as_ref() {
            let msg = ClientMessage::Signal {
                to_peer: to_peer.clone(),
                signal: signal.to_value()?,
            };
            self.signaling.send(msg).await?;
        } else {
            return Err(anyhow::anyhow!("Remote peer ID not set"));
        }
        Ok(())
    }
}
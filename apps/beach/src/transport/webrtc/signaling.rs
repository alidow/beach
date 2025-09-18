use anyhow::Result;
use serde::{Deserialize, Serialize};
use webrtc::ice_transport::ice_candidate::RTCIceCandidate;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

/// WebRTC signaling messages for SDP and ICE exchange
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SignalingMessage {
    /// SDP offer from the initiator
    Offer { sdp: String },
    /// SDP answer from the responder
    Answer { sdp: String },
    /// ICE candidate for connection establishment
    IceCandidate {
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    },
}

impl SignalingMessage {
    /// Create an offer message from RTCSessionDescription
    pub fn from_offer(desc: &RTCSessionDescription) -> Self {
        SignalingMessage::Offer {
            sdp: desc.sdp.clone(),
        }
    }

    /// Create an answer message from RTCSessionDescription
    pub fn from_answer(desc: &RTCSessionDescription) -> Self {
        SignalingMessage::Answer {
            sdp: desc.sdp.clone(),
        }
    }

    /// Create an ICE candidate message
    pub fn from_ice_candidate(candidate: &RTCIceCandidate) -> Self {
        SignalingMessage::IceCandidate {
            candidate: candidate.to_json().unwrap_or_default().candidate,
            sdp_mid: candidate.to_json().ok().and_then(|c| c.sdp_mid),
            sdp_mline_index: candidate.to_json().ok().and_then(|c| c.sdp_mline_index),
        }
    }

    /// Convert to RTCSessionDescription for offers/answers
    pub fn to_session_description(&self) -> Result<RTCSessionDescription> {
        match self {
            SignalingMessage::Offer { sdp } => Ok(RTCSessionDescription::offer(sdp.clone())?),
            SignalingMessage::Answer { sdp } => Ok(RTCSessionDescription::answer(sdp.clone())?),
            _ => Err(anyhow::anyhow!("Not a session description message")),
        }
    }
}

/// Simple in-memory signaling channel for testing
#[derive(Clone)]
pub struct LocalSignalingChannel {
    tx: tokio::sync::mpsc::UnboundedSender<SignalingMessage>,
    rx: std::sync::Arc<tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<SignalingMessage>>>,
}

impl LocalSignalingChannel {
    /// Create a pair of connected signaling channels
    pub fn create_pair() -> (Self, Self) {
        let (tx1, rx1) = tokio::sync::mpsc::unbounded_channel();
        let (tx2, rx2) = tokio::sync::mpsc::unbounded_channel();

        let channel1 = Self {
            tx: tx2,
            rx: std::sync::Arc::new(tokio::sync::Mutex::new(rx1)),
        };

        let channel2 = Self {
            tx: tx1,
            rx: std::sync::Arc::new(tokio::sync::Mutex::new(rx2)),
        };

        (channel1, channel2)
    }

    /// Send a signaling message
    pub async fn send(&self, message: SignalingMessage) -> Result<()> {
        self.tx
            .send(message)
            .map_err(|e| anyhow::anyhow!("Failed to send signaling message: {}", e))?;
        Ok(())
    }

    /// Receive a signaling message
    pub async fn recv(&self) -> Option<SignalingMessage> {
        let mut rx = self.rx.lock().await;
        rx.recv().await
    }
}

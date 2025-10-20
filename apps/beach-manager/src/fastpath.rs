use std::{collections::HashMap, sync::Arc};

use beach_buggy::{ActionAck, ActionCommand, StateDiff};
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{info, warn};
use uuid::Uuid;

use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::sdp_type::RTCSdpType;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::Error as WebRtcError;

use crate::state::AppState;

#[derive(Clone)]
pub struct FastPathSession {
    pub session_id: String,
    pub pc: Arc<RTCPeerConnection>,
    pub actions_tx: Arc<Mutex<Option<Arc<RTCDataChannel>>>>,
    pub acks_rx: Arc<Mutex<Option<Arc<RTCDataChannel>>>>,
    pub state_rx: Arc<Mutex<Option<Arc<RTCDataChannel>>>>,
    // local ICE candidates gathered before answer is delivered
    pub local_ice: Arc<RwLock<Vec<serde_json::Value>>>,
}

impl FastPathSession {
    pub async fn new(session_id: String) -> Result<Self, WebRtcError> {
        let cfg = RTCConfiguration::default();
        let pc = webrtc::api::APIBuilder::new()
            .build()
            .new_peer_connection(cfg)
            .await?;

        Ok(FastPathSession {
            session_id,
            pc: Arc::new(pc),
            actions_tx: Arc::new(Mutex::new(None)),
            acks_rx: Arc::new(Mutex::new(None)),
            state_rx: Arc::new(Mutex::new(None)),
            local_ice: Arc::new(RwLock::new(Vec::new())),
        })
    }

    pub async fn set_remote_offer(
        &self,
        offer: RTCSessionDescription,
    ) -> Result<RTCSessionDescription, WebRtcError> {
        self.pc.set_remote_description(offer).await?;

        let this = self.clone();
        self.pc
            .on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
                let label = dc.label().to_string();
                let this2 = this.clone();
                Box::pin(async move {
                    info!(label = %label, "fast-path data channel opened");
                    match label.as_str() {
                        "mgr-actions" => {
                            *this2.actions_tx.lock().await = Some(dc.clone());
                        }
                        "mgr-acks" => {
                            *this2.acks_rx.lock().await = Some(dc.clone());
                        }
                        "mgr-state" => {
                            *this2.state_rx.lock().await = Some(dc.clone());
                        }
                        _ => {}
                    }
                })
            }));

        let this = self.clone();
        self.pc.on_ice_candidate(Box::new(move |c| {
            let this3 = this.clone();
            Box::pin(async move {
                if let Some(cand) = c {
                    if let Ok(json) = cand.to_json() {
                        let val = serde_json::json!({
                            "candidate": json.candidate,
                            "sdp_mid": json.sdp_mid,
                            "sdp_mline_index": json.sdp_mline_index,
                        });
                        this3.local_ice.write().await.push(val);
                    }
                }
            })
        }));

        let answer = self.pc.create_answer(None).await?;
        self.pc.set_local_description(answer.clone()).await?;
        Ok(answer)
    }

    pub async fn add_remote_ice(&self, cand: RTCIceCandidateInit) -> Result<(), WebRtcError> {
        self.pc.add_ice_candidate(cand).await
    }
}

#[derive(Clone, Default)]
pub struct FastPathRegistry {
    inner: Arc<RwLock<HashMap<String, Arc<FastPathSession>>>>,
}

impl FastPathRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(&self, session_id: String, fps: Arc<FastPathSession>) {
        self.inner.write().await.insert(session_id, fps);
    }

    pub async fn get(&self, session_id: &str) -> Option<Arc<FastPathSession>> {
        self.inner.read().await.get(session_id).cloned()
    }
}

pub async fn send_actions_over_fast_path(
    registry: &FastPathRegistry,
    session_id: &str,
    actions: &[ActionCommand],
) -> anyhow::Result<bool> {
    if let Some(fps) = registry.get(session_id).await {
        let guard = fps.actions_tx.lock().await;
        if let Some(dc) = guard.as_ref() {
            for a in actions {
                let text =
                    serde_json::to_string(&serde_json::json!({"type":"action","payload":a}))?;
                dc.send_text(text)
                    .await
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            }
            return Ok(true);
        }
    }
    Ok(false)
}

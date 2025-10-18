use axum::{extract::{Path, State}, Json};
use serde::Deserialize;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription as _;
use webrtc::peer_connection::sdp::session_description::*;
use webrtc::peer_connection::sdp::sdp_type::RTCSdpType;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;

use crate::{routes::{ApiError, ApiResult, AuthToken}, state::AppState};
pub use crate::fastpath::FastPathSession;
pub use crate::fastpath::{send_actions_over_fast_path, FastPathRegistry};

#[derive(Deserialize)]
pub struct OfferBody {
    pub sdp: String,
    pub r#type: String,
}

pub async fn answer_offer(
    State(state): State<AppState>,
    token: AuthToken,
    Path(session_id): Path<String>,
    Json(body): Json<OfferBody>,
) -> ApiResult<serde_json::Value> {
    // Harness publishes; require publish scope
    crate::routes::sessions::ensure_scope(&token, "pb:harness.publish").map_err(|e| e)?;
    let offer = RTCSessionDescription::offer(body.sdp)
        .map_err(|e| ApiError::BadRequest(format!("invalid offer: {e}")))?;
    let fps = FastPathSession::new(session_id.clone())
        .await
        .map_err(|e| ApiError::BadRequest(format!("webrtc error: {e}")))?;
    let answer = fps
        .set_remote_offer(offer)
        .await
        .map_err(|e| ApiError::BadRequest(format!("webrtc error: {e}")))?;
    state.attach_fast_path(session_id, fps).await;
    Ok(Json(serde_json::json!({ "sdp": answer.sdp })))
}

#[derive(Deserialize)]
pub struct IceBody {
    pub candidate: String,
    pub sdp_mid: Option<String>,
    pub sdp_mline_index: Option<u16>,
}

pub async fn add_remote_ice(
    State(state): State<AppState>,
    token: AuthToken,
    Path(session_id): Path<String>,
    Json(body): Json<IceBody>,
) -> ApiResult<serde_json::Value> {
    crate::routes::sessions::ensure_scope(&token, "pb:harness.publish").map_err(|e| e)?;
    let fps = state
        .fast_path_for(&session_id)
        .await
        .ok_or(ApiError::NotFound("fast path not found"))?;
    let init = RTCIceCandidateInit {
        candidate: body.candidate,
        sdp_mid: body.sdp_mid,
        sdp_mline_index: body.sdp_mline_index,
        ..Default::default()
    };
    fps.add_remote_ice(init)
        .await
        .map_err(|e| ApiError::BadRequest(format!("webrtc error: {e}")))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn list_local_ice(
    State(state): State<AppState>,
    token: AuthToken,
    Path(session_id): Path<String>,
) -> ApiResult<serde_json::Value> {
    crate::routes::sessions::ensure_scope(&token, "pb:harness.publish").map_err(|e| e)?;
    let fps = state
        .fast_path_for(&session_id)
        .await
        .ok_or(ApiError::NotFound("fast path not found"))?;
    let list = fps.local_ice.read().await.clone();
    Ok(Json(serde_json::json!({ "candidates": list })))
}

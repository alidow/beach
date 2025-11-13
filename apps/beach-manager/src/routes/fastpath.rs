use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

use crate::fastpath::FastPathSession;
use crate::{
    routes::{ApiError, ApiResult, AuthToken},
    state::AppState,
};

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
    if !body.r#type.eq_ignore_ascii_case("offer") {
        return Err(ApiError::BadRequest(format!(
            "expected SDP type 'offer', got '{}'",
            body.r#type
        )));
    }
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

#[derive(Deserialize, Default)]
pub struct AnswerQuery {
    pub handshake_id: Option<String>,
    pub to_peer: Option<String>,
    pub from_peer: Option<String>,
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

pub async fn get_local_answer(
    State(state): State<AppState>,
    token: AuthToken,
    Path(session_id): Path<String>,
    Query(query): Query<AnswerQuery>,
) -> ApiResult<serde_json::Value> {
    crate::routes::sessions::ensure_scope(&token, "pb:harness.publish").map_err(|e| e)?;
    let fps = state
        .fast_path_for(&session_id)
        .await
        .ok_or(ApiError::NotFound("fast path not found"))?;
    let Some(answer) = fps.local_description().await else {
        return Err(ApiError::NotFound("fast path answer not ready"));
    };
    let sdp = answer.sdp;
    let typ = answer.sdp_type.to_string();
    let payload = serde_json::json!({
        "sdp": sdp,
        "type": typ,
        "handshake_id": query.handshake_id.unwrap_or_else(|| "fastpath".into()),
        "from_peer": query.from_peer.unwrap_or_else(|| "private-beach-manager".into()),
        "to_peer": query.to_peer.unwrap_or_else(|| "fastpath-client".into())
    });
    Ok(Json(payload))
}

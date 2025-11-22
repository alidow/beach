use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    Json,
};
use serde::Deserialize;
use tracing::info;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

use crate::fastpath::FastPathSession;
use crate::routes::sessions::{claims_has_scope, extract_bearer};
use crate::{
    routes::{ApiError, ApiResult},
    state::AppState,
};

#[derive(Deserialize)]
pub struct OfferBody {
    pub sdp: String,
    pub r#type: String,
}

pub async fn answer_offer(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(body): Json<OfferBody>,
) -> ApiResult<serde_json::Value> {
    authorize_fast_path(&state, &headers, &session_id).await?;
    // Harness publishes; require publish scope
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
    // Bind state before answering the offer so early data channel events
    // (which may fire before we spawn the receivers) can install handlers.
    fps.preload_state(state.clone()).await;
    let answer = fps
        .set_remote_offer(offer)
        .await
        .map_err(|e| ApiError::BadRequest(format!("webrtc error: {e}")))?;
    info!(
        target = "controller.fast_path_state",
        session_id = %session_id,
        fast_path_id = fps.instance_id(),
        "fast-path offer answered"
    );
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
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(body): Json<IceBody>,
) -> ApiResult<serde_json::Value> {
    authorize_fast_path(&state, &headers, &session_id).await?;
    let fps = state
        .fast_path_for(&session_id)
        .await
        .ok_or(ApiError::NotFound("fast path not found"))?;
    let IceBody {
        candidate,
        sdp_mid,
        sdp_mline_index,
    } = body;
    let init = RTCIceCandidateInit {
        candidate: candidate.clone(),
        sdp_mid: sdp_mid.clone(),
        sdp_mline_index,
        ..Default::default()
    };
    fps.add_remote_ice(init)
        .await
        .map_err(|e| ApiError::BadRequest(format!("webrtc error: {e}")))?;
    info!(
        target = "controller.fast_path_state",
        session_id = %session_id,
        fast_path_id = fps.instance_id(),
        sdp_mid = sdp_mid.as_deref(),
        sdp_mline_index,
        "fast-path remote ICE candidate applied"
    );
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn list_local_ice(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> ApiResult<serde_json::Value> {
    authorize_fast_path(&state, &headers, &session_id).await?;
    let fps = state
        .fast_path_for(&session_id)
        .await
        .ok_or(ApiError::NotFound("fast path not found"))?;
    let list = fps.local_ice.read().await.clone();
    Ok(Json(serde_json::json!({ "candidates": list })))
}

pub async fn get_local_answer(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Query(query): Query<AnswerQuery>,
) -> ApiResult<serde_json::Value> {
    authorize_fast_path(&state, &headers, &session_id).await?;
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
    info!(
        target = "controller.fast_path_state",
        session_id = %session_id,
        fast_path_id = fps.instance_id(),
        handshake_id = payload["handshake_id"].as_str(),
        from_peer = payload["from_peer"].as_str(),
        to_peer = payload["to_peer"].as_str(),
        "fast-path answer served"
    );
    Ok(Json(payload))
}

async fn authorize_fast_path(
    state: &AppState,
    headers: &HeaderMap,
    session_id: &str,
) -> Result<(), ApiError> {
    let Some(bearer) = extract_bearer(headers) else {
        return Err(ApiError::Unauthorized);
    };
    if state
        .publish_token_manager()
        .verify_for_session(&bearer, session_id)
        .is_ok()
    {
        return Ok(());
    }

    let claims = state
        .auth_context()
        .verify_strict(&bearer)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    if !claims_has_scope(&claims, "pb:harness.publish") {
        return Err(ApiError::Forbidden("pb:harness.publish"));
    }
    Ok(())
}

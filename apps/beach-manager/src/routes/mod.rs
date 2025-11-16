mod auth;
pub mod fastpath;
mod mcp;
mod private_beaches;
mod sessions;
mod sse;

use axum::{
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
    Json, Router,
};
use serde::Serialize;
use tower_http::cors::{Any, CorsLayer};

use crate::state::AppState;

pub use auth::AuthToken;
pub use private_beaches::*;
pub use sessions::*;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health_check))
        .route("/readyz", get(health_check))
        .route("/metrics", get(sse::prometheus_metrics))
        .route("/sessions/register", post(register_session))
        .route("/sessions/:session_id", patch(update_session))
        .route("/sessions/:session_id/join", post(join_session))
        .route(
            "/sessions/:session_id/state",
            get(fetch_state_snapshot).post(push_state),
        )
        .route("/sessions/:session_id/state/stream", get(sse::stream_state))
        .route("/sessions/:session_id/actions", post(queue_actions))
        .route("/sessions/:session_id/actions/poll", get(poll_actions))
        .route(
            "/sessions/:session_id/actions/pending",
            get(pending_actions),
        )
        .route("/sessions/:session_id/actions/ack", post(ack_actions))
        .route(
            "/sessions/:session_id/transport-status",
            post(update_transport_status),
        )
        .route(
            "/sessions/:session_id/controller/lease",
            post(acquire_controller).delete(release_controller),
        )
        .route(
            "/sessions/:session_id/controller-handshake",
            post(issue_controller_handshake).delete(revoke_controller_handshake),
        )
        .route(
            "/sessions/:session_id/controller-events",
            get(list_controller_events),
        )
        .route(
            "/sessions/:controller_id/controllers",
            get(list_controller_pairings_route).post(create_controller_pairing),
        )
        .route(
            "/sessions/:controller_id/controllers/stream",
            get(sse::stream_controller_pairings),
        )
        .route(
            "/sessions/:controller_id/controllers/:child_session_id",
            delete(delete_controller_pairing),
        )
        .route("/sessions/:session_id/health", post(signal_health))
        .route(
            "/private-beaches/:private_beach_id/sessions",
            get(list_sessions),
        )
        .route(
            "/private-beaches/:private_beach_id/sessions/attach-by-code",
            post(attach_by_code),
        )
        .route(
            "/private-beaches/:private_beach_id/sessions/attach",
            post(attach_owned),
        )
        .route(
            "/private-beaches/:private_beach_id/sessions/:session_id/viewer-credential",
            get(get_viewer_credential),
        )
        .route("/sessions/:session_id/emergency-stop", post(emergency_stop))
        .route("/agents/onboard", post(onboard_agent))
        .route(
            "/fastpath/sessions/:session_id/webrtc/offer",
            post(fastpath::answer_offer),
        )
        .route(
            "/fastpath/sessions/:session_id/webrtc/answer",
            get(fastpath::get_local_answer),
        )
        .route(
            "/fastpath/sessions/:session_id/webrtc/ice",
            post(fastpath::add_remote_ice).get(fastpath::list_local_ice),
        )
        .route("/mcp", post(mcp::handle_mcp))
        // Private Beaches CRUD + layout
        .route(
            "/private-beaches",
            get(list_private_beaches).post(create_private_beach),
        )
        .route(
            "/private-beaches/:id",
            get(get_private_beach).patch(update_private_beach),
        )
        .route(
            "/private-beaches/:id/layout",
            get(get_private_beach_layout).put(put_private_beach_layout),
        )
        .route(
            "/private-beaches/:id/controller-assignments/batch",
            post(batch_controller_assignments),
        )
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(state)
}

async fn health_check() -> &'static str {
    "ok"
}

pub type ApiResult<T> = Result<Json<T>, ApiError>;

#[derive(Debug)]
pub enum ApiError {
    Unauthorized,
    Forbidden(&'static str),
    NotFound(&'static str),
    Conflict(&'static str),
    ConflictWithCode {
        message: &'static str,
        code: &'static str,
    },
    PreconditionFailed {
        message: &'static str,
        code: &'static str,
    },
    TooManyRequests(&'static str),
    BadRequest(String),
    Upstream(&'static str),
    Internal,
}

#[derive(Debug, Serialize)]
struct ApiErrorBody<'a> {
    error: &'a str,
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_code: Option<String>,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::Unauthorized => (
                axum::http::StatusCode::UNAUTHORIZED,
                Json(ApiErrorBody {
                    error: "unauthorized",
                    message: None,
                    error_code: None,
                }),
            )
                .into_response(),
            ApiError::TooManyRequests(msg) => (
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                Json(ApiErrorBody {
                    error: "too_many_requests",
                    message: Some(msg.to_string()),
                    error_code: None,
                }),
            )
                .into_response(),
            ApiError::Forbidden(msg) => (
                axum::http::StatusCode::FORBIDDEN,
                Json(ApiErrorBody {
                    error: "forbidden",
                    message: Some(msg.to_string()),
                    error_code: None,
                }),
            )
                .into_response(),
            ApiError::NotFound(msg) => (
                axum::http::StatusCode::NOT_FOUND,
                Json(ApiErrorBody {
                    error: "not_found",
                    message: Some(msg.to_string()),
                    error_code: None,
                }),
            )
                .into_response(),
            ApiError::Conflict(msg) => (
                axum::http::StatusCode::CONFLICT,
                Json(ApiErrorBody {
                    error: "conflict",
                    message: Some(msg.to_string()),
                    error_code: None,
                }),
            )
                .into_response(),
            ApiError::ConflictWithCode { message, code } => (
                axum::http::StatusCode::CONFLICT,
                Json(ApiErrorBody {
                    error: "conflict",
                    message: Some(message.to_string()),
                    error_code: Some(code.to_string()),
                }),
            )
                .into_response(),
            ApiError::PreconditionFailed { message, code } => (
                axum::http::StatusCode::PRECONDITION_FAILED,
                Json(ApiErrorBody {
                    error: "precondition_failed",
                    message: Some(message.to_string()),
                    error_code: Some(code.to_string()),
                }),
            )
                .into_response(),
            ApiError::BadRequest(msg) => (
                axum::http::StatusCode::BAD_REQUEST,
                Json(ApiErrorBody {
                    error: "bad_request",
                    message: Some(msg),
                    error_code: None,
                }),
            )
                .into_response(),
            ApiError::Upstream(msg) => (
                axum::http::StatusCode::BAD_GATEWAY,
                Json(ApiErrorBody {
                    error: "upstream_error",
                    message: Some(msg.to_string()),
                    error_code: None,
                }),
            )
                .into_response(),
            ApiError::Internal => (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiErrorBody {
                    error: "internal_error",
                    message: Some("internal server error".into()),
                    error_code: None,
                }),
            )
                .into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::SessionSummary;
    use axum::{
        body::{self, Body},
        http::{Request, StatusCode},
    };
    use beach_buggy::{AckStatus, ActionAck, ActionCommand, HarnessType, RegisterSessionResponse};
    use serde_json::json;
    use std::time::SystemTime;
    use tower::util::ServiceExt;

    #[tokio::test]
    async fn harness_register_and_action_flow() {
        let state = AppState::new();
        let app = build_router(state.clone());

        let register_body = json!({
            "session_id": "sess-test",
            "private_beach_id": "pb-test",
            "harness_type": HarnessType::TerminalShim,
            "capabilities": ["terminal_diff_v1"],
            "location_hint": "us-test-1",
            "metadata": { "tag": "demo" },
            "version": "0.1.0"
        });

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions/register")
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "application/json")
                    .body(Body::from(register_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let register_resp: RegisterSessionResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(register_resp.harness_id.len(), 36);
        let controller_token = register_resp.controller_token.unwrap();

        let queue_body = json!({
            "controller_token": controller_token,
            "actions": [{
                "id": "cmd-demo",
                "action_type": "terminal_write",
                "payload": { "bytes": "hello" }
            }]
        });
        let queue_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions/sess-test/actions")
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "application/json")
                    .body(Body::from(queue_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(queue_resp.status(), StatusCode::OK);

        let poll_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/sessions/sess-test/actions/poll")
                    .header("authorization", "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(poll_resp.status(), StatusCode::OK);
        let bytes = body::to_bytes(poll_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let commands: Vec<ActionCommand> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].id, "cmd-demo");

        let ack = ActionAck {
            id: "cmd-demo".into(),
            status: AckStatus::Ok,
            applied_at: SystemTime::now(),
            latency_ms: Some(5),
            error_code: None,
            error_message: None,
        };
        let ack_body = serde_json::to_string(&vec![ack]).unwrap();
        let ack_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions/sess-test/actions/ack")
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "application/json")
                    .body(Body::from(ack_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ack_resp.status(), StatusCode::OK);

        let list_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/private-beaches/pb-test/sessions")
                    .header("authorization", "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list_resp.status(), StatusCode::OK);
        let bytes = body::to_bytes(list_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let summaries: Vec<SessionSummary> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].session_id, "sess-test");
    }

    #[tokio::test]
    async fn controller_token_query_allows_poll_and_ack() {
        let state = AppState::new();
        let app = build_router(state.clone());

        let register_body = json!({
            "session_id": "sess-query",
            "private_beach_id": "pb-query",
            "harness_type": HarnessType::TerminalShim,
            "capabilities": ["terminal_diff_v1"],
            "location_hint": null,
            "metadata": null,
            "version": "0.1.0"
        });

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions/register")
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "application/json")
                    .body(Body::from(register_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let register_resp: RegisterSessionResponse = serde_json::from_slice(&bytes).unwrap();
        let controller_token = register_resp
            .controller_token
            .clone()
            .expect("controller token present");

        let queue_body = json!({
            "controller_token": controller_token,
            "actions": [{
                "id": "cmd-query",
                "action_type": "terminal_write",
                "payload": { "bytes": "ping" }
            }]
        });
        let queue_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions/sess-query/actions")
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "application/json")
                    .body(Body::from(queue_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(queue_resp.status(), StatusCode::OK);

        let poll_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/sessions/sess-query/actions/poll?controller_token={}",
                        controller_token
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(poll_resp.status(), StatusCode::OK);
        let bytes = body::to_bytes(poll_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let commands: Vec<ActionCommand> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].id, "cmd-query");

        let ack = ActionAck {
            id: "cmd-query".into(),
            status: AckStatus::Ok,
            applied_at: SystemTime::now(),
            latency_ms: Some(4),
            error_code: None,
            error_message: None,
        };
        let ack_body = serde_json::to_string(&vec![ack]).unwrap();
        let ack_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/sessions/sess-query/actions/ack?controller_token={}",
                        controller_token
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(ack_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ack_resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn controller_pairing_crud() {
        let state = AppState::new();
        let app = build_router(state.clone());

        let controller_register = json!({
            "session_id": "controller-1",
            "private_beach_id": "pb-ctrl",
            "harness_type": HarnessType::TerminalShim,
            "capabilities": ["terminal_diff_v1"],
            "version": "0.1.0"
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions/register")
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "application/json")
                    .body(Body::from(controller_register.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let child_register = json!({
            "session_id": "child-1",
            "private_beach_id": "pb-ctrl",
            "harness_type": HarnessType::TerminalShim,
            "capabilities": ["terminal_diff_v1"],
            "version": "0.1.0"
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions/register")
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "application/json")
                    .body(Body::from(child_register.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let pairing_body = json!({
            "child_session_id": "child-1",
            "prompt_template": "Focus on shell commands",
            "update_cadence": "fast"
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions/controller-1/controllers")
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "application/json")
                    .body(Body::from(pairing_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let pairing_resp: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(pairing_resp["child_session_id"], "child-1");
        assert_eq!(pairing_resp["update_cadence"], "fast");
        assert_eq!(pairing_resp["transport_status"]["transport"], "pending");

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/sessions/controller-1/controllers")
                    .header("authorization", "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let pairings: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(pairings.len(), 1);
        assert_eq!(pairings[0]["child_session_id"], "child-1");
        assert_eq!(pairings[0]["transport_status"]["transport"], "pending");

        let update_body = json!({"transport": "fast_path"});
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions/controller-1/transport-status")
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "application/json")
                    .body(Body::from(update_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/sessions/controller-1/controllers")
                    .header("authorization", "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let pairings: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(pairings[0]["transport_status"]["transport"], "fast_path");

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/sessions/controller-1/controllers/child-1")
                    .header("authorization", "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/sessions/controller-1/controllers")
                    .header("authorization", "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let pairings: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
        assert!(pairings.is_empty());
    }

    #[tokio::test]
    async fn batch_controller_assignments_endpoint() {
        let state = AppState::new();
        let app = build_router(state.clone());

        let controller_register = json!({
            "session_id": "controller-batch",
            "private_beach_id": "pb-batch",
            "harness_type": HarnessType::TerminalShim,
            "capabilities": ["terminal_diff_v1"],
            "version": "0.1.0"
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions/register")
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "application/json")
                    .body(Body::from(controller_register.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let lease_request = json!({ "reason": "batch-test" });
        let lease_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions/controller-batch/controller/lease")
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "application/json")
                    .body(Body::from(lease_request.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(lease_resp.status(), StatusCode::OK);

        let child_register = json!({
            "session_id": "child-batch",
            "private_beach_id": "pb-batch",
            "harness_type": HarnessType::TerminalShim,
            "capabilities": ["terminal_diff_v1"],
            "version": "0.1.0"
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions/register")
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "application/json")
                    .body(Body::from(child_register.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let batch_body = json!({
            "assignments": [
                { "controller_session_id": "controller-batch", "child_session_id": "child-batch" },
                { "controller_session_id": "controller-batch", "child_session_id": "missing-child" }
            ]
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/private-beaches/pb-batch/controller-assignments/batch")
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "application/json")
                    .body(Body::from(batch_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let results: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let array = results["results"].as_array().unwrap();
        assert_eq!(array.len(), 2);
        assert!(array[0]["ok"].as_bool().unwrap());
        assert!(!array[1]["ok"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn mcp_register_and_list_sessions() {
        let state = AppState::new();
        let app = build_router(state.clone());

        let register_rpc = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "private_beach.register_session",
            "params": {
                "session_id": "6a7a7d0a-1b8b-4d80-8c13-111111111111",
                "private_beach_id": "ec1a9f74-91ff-4511-9cd8-222222222222",
                "harness_type": "terminal_shim",
                "capabilities": ["terminal_diff_v1"],
                "location_hint": "us-test-1",
                "metadata": { "tag": "demo" },
                "version": "0.1.0"
            }
        });

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "application/json")
                    .body(Body::from(register_rpc.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let rpc_resp: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(rpc_resp.get("error").is_none());
        let controller_token = rpc_resp["result"]["controller_token"]
            .as_str()
            .expect("controller token present")
            .to_string();
        assert!(!controller_token.is_empty());

        let list_rpc = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "private_beach.list_sessions",
            "params": {
                "private_beach_id": "ec1a9f74-91ff-4511-9cd8-222222222222"
            }
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "application/json")
                    .body(Body::from(list_rpc.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let rpc_resp: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(rpc_resp.get("error").is_none());
        let sessions = rpc_resp["result"].as_array().expect("sessions array");
        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0]["session_id"],
            "6a7a7d0a-1b8b-4d80-8c13-111111111111"
        );
    }

    #[tokio::test]
    async fn controller_handshake_endpoint_happy_path() {
        // Skip external verify for tests
        unsafe {
            std::env::set_var("BEACH_SKIP_ROAD_VERIFY", "1");
        }
        let state = AppState::new();
        let app = build_router(state.clone());
        let session_id = "sess-handshake";
        let beach_id = "pb-handshake";
        let body = json!({
            "passcode": "123456",
            "requester_private_beach_id": beach_id,
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/sessions/{}/controller-handshake", session_id))
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(payload["private_beach_id"], beach_id);
        assert!(payload["controller_token"].as_str().unwrap().len() > 0);
        // Clean up env var
        unsafe {
            std::env::remove_var("BEACH_SKIP_ROAD_VERIFY");
        }
    }
}

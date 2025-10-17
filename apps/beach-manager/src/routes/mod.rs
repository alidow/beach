mod auth;
mod mcp;
mod sessions;

use axum::{
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use serde::Serialize;

use crate::state::AppState;

pub use auth::AuthToken;
pub use sessions::*;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health_check))
        .route("/readyz", get(health_check))
        .route("/sessions/register", post(register_session))
        .route("/sessions/:session_id", patch(update_session))
        .route("/sessions/:session_id/state", post(push_state))
        .route("/sessions/:session_id/actions", post(queue_actions))
        .route("/sessions/:session_id/actions/poll", get(poll_actions))
        .route("/sessions/:session_id/actions/ack", post(ack_actions))
        .route(
            "/sessions/:session_id/controller/lease",
            post(acquire_controller).delete(release_controller),
        )
        .route(
            "/sessions/:session_id/controller-events",
            get(list_controller_events),
        )
        .route("/sessions/:session_id/health", post(signal_health))
        .route(
            "/private-beaches/:private_beach_id/sessions",
            get(list_sessions),
        )
        .route("/agents/onboard", post(onboard_agent))
        .route("/mcp", post(mcp::handle_mcp))
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
    BadRequest(String),
}

#[derive(Debug, Serialize)]
struct ApiErrorBody<'a> {
    error: &'a str,
    message: Option<String>,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::Unauthorized => (
                axum::http::StatusCode::UNAUTHORIZED,
                Json(ApiErrorBody {
                    error: "unauthorized",
                    message: None,
                }),
            )
                .into_response(),
            ApiError::Forbidden(msg) => (
                axum::http::StatusCode::FORBIDDEN,
                Json(ApiErrorBody {
                    error: "forbidden",
                    message: Some(msg.to_string()),
                }),
            )
                .into_response(),
            ApiError::NotFound(msg) => (
                axum::http::StatusCode::NOT_FOUND,
                Json(ApiErrorBody {
                    error: "not_found",
                    message: Some(msg.to_string()),
                }),
            )
                .into_response(),
            ApiError::Conflict(msg) => (
                axum::http::StatusCode::CONFLICT,
                Json(ApiErrorBody {
                    error: "conflict",
                    message: Some(msg.to_string()),
                }),
            )
                .into_response(),
            ApiError::BadRequest(msg) => (
                axum::http::StatusCode::BAD_REQUEST,
                Json(ApiErrorBody {
                    error: "bad_request",
                    message: Some(msg),
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
}

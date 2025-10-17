use axum::{extract::State, Json};
use beach_buggy::{ActionAck, ActionCommand, RegisterSessionRequest};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::error;
use uuid::Uuid;

use crate::state::{AppState, StateError};

use super::AuthToken;

fn require_scope(
    token: &AuthToken,
    id: &Option<Value>,
    scope: &'static str,
) -> Option<JsonRpcResponse> {
    if token.has_scope(scope) {
        None
    } else {
        Some(scope_error(id.clone(), scope))
    }
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(super) struct JsonRpcRequest {
    pub jsonrpc: Option<String>,
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Debug, Serialize)]
pub(super) struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub(super) struct JsonRpcError {
    code: i32,
    message: String,
}

#[derive(Debug, Deserialize)]
struct ListSessionsParams {
    private_beach_id: String,
}

#[derive(Debug, Deserialize)]
struct AcquireControllerParams {
    session_id: String,
    requesting_account_id: Option<String>,
    ttl_ms: Option<u64>,
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReleaseControllerParams {
    session_id: String,
    controller_token: String,
}

#[derive(Debug, Deserialize)]
struct QueueActionsParams {
    session_id: String,
    controller_token: String,
    actions: Vec<ActionCommand>,
}

#[derive(Debug, Deserialize)]
struct AckActionsParams {
    session_id: String,
    acks: Vec<ActionAck>,
}

pub async fn handle_mcp(
    State(state): State<AppState>,
    token: AuthToken,
    Json(request): Json<JsonRpcRequest>,
) -> Json<JsonRpcResponse> {
    let id = request.id.clone();
    let response = match request.method.as_str() {
        "private_beach.register_session" => {
            if let Some(resp) = require_scope(&token, &id, "pb:sessions.register") {
                return Json(resp);
            }
            match decode_params::<RegisterSessionRequest>(request.params) {
                Ok(params) => match state.register_session(params).await {
                    Ok(result) => success(id, result),
                    Err(err) => state_error(id, err),
                },
                Err(err) => invalid_params(id, err),
            }
        }
        "private_beach.list_sessions" => {
            if let Some(resp) = require_scope(&token, &id, "pb:sessions.read") {
                return Json(resp);
            }
            match decode_params::<ListSessionsParams>(request.params) {
                Ok(params) => match state.list_sessions(&params.private_beach_id).await {
                    Ok(rows) => success(id, rows),
                    Err(err) => state_error(id, err),
                },
                Err(err) => invalid_params(id, err),
            }
        }
        "private_beach.acquire_controller" => {
            if let Some(resp) = require_scope(&token, &id, "pb:control.write") {
                return Json(resp);
            }
            match decode_params::<AcquireControllerParams>(request.params) {
                Ok(params) => {
                    let requester = token.account_uuid().or_else(|| {
                        params
                            .requesting_account_id
                            .as_deref()
                            .and_then(|s| Uuid::parse_str(s).ok())
                    });
                    match state
                        .acquire_controller(
                            &params.session_id,
                            params.ttl_ms,
                            params.reason,
                            requester,
                        )
                        .await
                    {
                        Ok(resp) => success(id, resp),
                        Err(err) => state_error(id, err),
                    }
                }
                Err(err) => invalid_params(id, err),
            }
        }
        "private_beach.release_controller" => {
            if let Some(resp) = require_scope(&token, &id, "pb:control.write") {
                return Json(resp);
            }
            match decode_params::<ReleaseControllerParams>(request.params) {
                Ok(params) => match state
                    .release_controller(
                        &params.session_id,
                        &params.controller_token,
                        token.account_uuid(),
                    )
                    .await
                {
                    Ok(_) => success(id, serde_json::json!({ "released": true })),
                    Err(err) => state_error(id, err),
                },
                Err(err) => invalid_params(id, err),
            }
        }
        "private_beach.queue_action" => {
            if let Some(resp) = require_scope(&token, &id, "pb:control.write") {
                resp
            } else {
                match decode_params::<QueueActionsParams>(request.params) {
                    Ok(params) => match state
                        .queue_actions(
                            &params.session_id,
                            &params.controller_token,
                            params.actions,
                            token.account_uuid(),
                        )
                        .await
                    {
                        Ok(_) => success(id, serde_json::json!({ "accepted": true })),
                        Err(err) => state_error(id, err),
                    },
                    Err(err) => invalid_params(id, err),
                }
            }
        }
        "private_beach.ack_actions" => {
            if let Some(resp) = require_scope(&token, &id, "pb:control.consume") {
                resp
            } else {
                match decode_params::<AckActionsParams>(request.params) {
                    Ok(params) => match state
                        .ack_actions(&params.session_id, params.acks, token.account_uuid())
                        .await
                    {
                        Ok(_) => success(id, serde_json::json!({ "acknowledged": true })),
                        Err(err) => state_error(id, err),
                    },
                    Err(err) => invalid_params(id, err),
                }
            }
        }
        "private_beach.subscribe_state" | "private_beach.controller_events.stream" => {
            JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32001,
                    message: "streaming methods are not yet supported".into(),
                }),
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: "method not found".into(),
            }),
        },
    };

    Json(response)
}

fn success<T: Serialize>(id: Option<Value>, value: T) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(serde_json::to_value(value).unwrap_or(Value::Null)),
        error: None,
    }
}

fn invalid_params(id: Option<Value>, err: serde_json::Error) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcError {
            code: -32602,
            message: format!("invalid params: {err}"),
        }),
    }
}

fn state_error(id: Option<Value>, err: StateError) -> JsonRpcResponse {
    let (code, message) = match &err {
        StateError::SessionNotFound => (-32004, "session not found".into()),
        StateError::ControllerMismatch => (-32005, "controller mismatch".into()),
        StateError::InvalidIdentifier(reason) => (-32602, reason.clone()),
        StateError::Database(db_err) => {
            error!(error = %db_err, "database error while processing MCP request");
            (-32010, "database error".into())
        }
        StateError::Redis(redis_err) => {
            error!(error = %redis_err, "redis error while processing MCP request");
            (-32011, "redis error".into())
        }
        StateError::Serde(serde_err) => {
            error!(error = %serde_err, "serialization error while processing MCP request");
            (-32603, "serialization error".into())
        }
    };

    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcError { code, message }),
    }
}

fn scope_error(id: Option<Value>, scope: &str) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcError {
            code: -32003,
            message: format!("missing scope {scope}"),
        }),
    }
}

fn decode_params<T: for<'de> Deserialize<'de>>(
    params: Option<Value>,
) -> Result<T, serde_json::Error> {
    serde_json::from_value(params.unwrap_or(Value::Null))
}

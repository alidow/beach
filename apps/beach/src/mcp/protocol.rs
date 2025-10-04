use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const JSONRPC_VERSION: &str = "2.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcResponse {
    Result(JsonRpcResult),
    Error(JsonRpcErrorResponse),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonRpcResult {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonRpcErrorResponse {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub error: JsonRpcError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResult {
    pub fn new(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: Some(result),
        }
    }

    pub fn empty(id: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: None,
        }
    }
}

impl JsonRpcErrorResponse {
    pub fn new(
        id: Option<Value>,
        code: i64,
        message: impl Into<String>,
        data: Option<Value>,
    ) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            error: JsonRpcError {
                code,
                message: message.into(),
                data,
            },
        }
    }
}

pub const ERROR_PARSE: i64 = -32700;
pub const ERROR_INVALID_REQUEST: i64 = -32600;
pub const ERROR_METHOD_NOT_FOUND: i64 = -32601;
pub const ERROR_INVALID_PARAMS: i64 = -32602;
pub const ERROR_INTERNAL: i64 = -32603;

pub const ERROR_UNAUTHORIZED: i64 = -32001;
pub const ERROR_CONFLICT: i64 = -32002;

pub fn method_not_found(id: Option<Value>, method: &str) -> JsonRpcResponse {
    JsonRpcResponse::Error(JsonRpcErrorResponse::new(
        id,
        ERROR_METHOD_NOT_FOUND,
        format!("method '{method}' not found"),
        None,
    ))
}

pub fn invalid_params(id: Option<Value>, message: impl Into<String>) -> JsonRpcResponse {
    JsonRpcResponse::Error(JsonRpcErrorResponse::new(
        id,
        ERROR_INVALID_PARAMS,
        message,
        None,
    ))
}

pub fn internal_error(id: Option<Value>, message: impl Into<String>) -> JsonRpcResponse {
    JsonRpcResponse::Error(JsonRpcErrorResponse::new(id, ERROR_INTERNAL, message, None))
}

pub fn unauthorized(id: Option<Value>, message: impl Into<String>) -> JsonRpcResponse {
    JsonRpcResponse::Error(JsonRpcErrorResponse::new(
        id,
        ERROR_UNAUTHORIZED,
        message,
        None,
    ))
}

pub fn conflict(id: Option<Value>, message: impl Into<String>) -> JsonRpcResponse {
    JsonRpcResponse::Error(JsonRpcErrorResponse::new(id, ERROR_CONFLICT, message, None))
}

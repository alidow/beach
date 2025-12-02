use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::io::{self, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::mcp::McpConfig;
use crate::mcp::auth::LeaseManager;
use crate::mcp::protocol::{
    JSONRPC_VERSION, JsonRpcRequest, JsonRpcResponse, JsonRpcResult, internal_error,
    invalid_params, method_not_found, unauthorized,
};
use crate::mcp::registry::{SessionRegistry, TerminalSession, global_registry};
use crate::mcp::terminal::TerminalSurface;

pub struct McpServer {
    config: McpConfig,
    service: Arc<McpService>,
}

struct SocketCleanup(PathBuf);

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        if let Err(err) = fs::remove_file(&self.0) {
            if err.kind() != ErrorKind::NotFound {
                warn!(path = %self.0.display(), error = %err, "failed to clean mcp socket");
            }
        }
    }
}

impl McpServer {
    pub fn new(config: McpConfig) -> Self {
        let read_only = config.effective_read_only();
        let registry = global_registry().clone();
        let leases = Arc::new(LeaseManager::new(read_only));
        let service = Arc::new(McpService::new(config.clone(), registry, leases));
        Self { config, service }
    }

    pub fn handle(&self) -> McpServerHandle {
        McpServerHandle {
            service: Arc::clone(&self.service),
        }
    }

    pub async fn run(self) -> Result<()> {
        if self.config.use_stdio {
            self.run_stdio().await
        } else {
            self.run_socket().await
        }
    }

    async fn run_stdio(self) -> Result<()> {
        let service = Arc::clone(&self.service);
        let stdin = io::stdin();
        let stdout = io::stdout();
        handle_connection(stdin, stdout, service).await;
        Ok(())
    }

    async fn run_socket(self) -> Result<()> {
        let path = self
            .config
            .socket
            .clone()
            .ok_or_else(|| anyhow::anyhow!("MCP socket path missing"))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create socket dir {parent:?}"))?;
        }
        if path.exists() {
            fs::remove_file(&path).with_context(|| format!("remove existing socket {path:?}"))?;
        }
        let listener =
            UnixListener::bind(&path).with_context(|| format!("bind MCP socket at {path:?}"))?;
        let _cleanup = SocketCleanup(path.clone());
        info!(socket = %path.display(), "MCP server listening");

        let service = Arc::clone(&self.service);

        loop {
            let (stream, addr) = listener.accept().await?;
            debug!(?addr, "accepted MCP connection");
            let service = service.clone();
            tokio::spawn(async move {
                handle_unix_stream(stream, service).await;
            });
        }
    }
}

#[derive(Clone)]
pub struct McpServerHandle {
    service: Arc<McpService>,
}

impl McpServerHandle {
    pub fn spawn_connection<R, W>(&self, reader: R, writer: W) -> JoinHandle<()>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let service = Arc::clone(&self.service);
        tokio::spawn(async move {
            handle_connection(reader, writer, service).await;
        })
    }
}

async fn handle_unix_stream(stream: UnixStream, service: Arc<McpService>) {
    let (read_half, write_half) = stream.into_split();
    handle_connection(read_half, write_half, service).await;
}

async fn handle_connection<R, W>(reader: R, writer: W, service: Arc<McpService>)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (tx, mut rx) = mpsc::channel::<serde_json::Value>(128);
    let writer_task = tokio::spawn(async move {
        write_loop(writer, &mut rx).await;
    });

    let state = Arc::new(ConnectionState::new(tx.clone()));
    let mut reader = BufReader::new(reader);

    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<serde_json::Value>(trimmed) {
                    Ok(value) => match serde_json::from_value::<JsonRpcRequest>(value.clone()) {
                        Ok(request) => {
                            if request.jsonrpc != JSONRPC_VERSION {
                                if let Some(id) = request.id.clone() {
                                    let response =
                                        invalid_params(Some(id), "jsonrpc version must be 2.0");
                                    let _ = tx.send(serde_json::to_value(response).unwrap()).await;
                                }
                                continue;
                            }
                            let response = service.handle_request(&state, request).await;
                            if let Some(response) = response {
                                if let Ok(value) = serde_json::to_value(response) {
                                    if tx.send(value).await.is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            warn!(error = %err, "invalid JSON-RPC request");
                            let response = JsonRpcResponse::Error(
                                crate::mcp::protocol::JsonRpcErrorResponse::new(
                                    None,
                                    crate::mcp::protocol::ERROR_INVALID_REQUEST,
                                    "invalid request",
                                    Some(value),
                                ),
                            );
                            if tx
                                .send(serde_json::to_value(response).unwrap())
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    },
                    Err(err) => {
                        warn!(error = %err, "failed to parse JSON payload");
                        let response = crate::mcp::protocol::JsonRpcResponse::Error(
                            crate::mcp::protocol::JsonRpcErrorResponse::new(
                                None,
                                crate::mcp::protocol::ERROR_PARSE,
                                "invalid json",
                                None,
                            ),
                        );
                        if tx
                            .send(serde_json::to_value(response).unwrap())
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            Err(err) => {
                warn!(error = %err, "connection read error");
                break;
            }
        }
    }

    state.shutdown().await;
    writer_task.abort();
}

async fn write_loop<W>(mut writer: W, rx: &mut mpsc::Receiver<serde_json::Value>)
where
    W: AsyncWrite + Unpin,
{
    while let Some(message) = rx.recv().await {
        match serde_json::to_string(&message) {
            Ok(mut text) => {
                text.push('\n');
                if writer.write_all(text.as_bytes()).await.is_err() {
                    break;
                }
                if writer.flush().await.is_err() {
                    break;
                }
            }
            Err(err) => {
                error!(error = %err, "failed to serialize json");
            }
        }
    }
}

struct SubscriptionEntry {
    cancel: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

struct ConnectionState {
    outgoing: mpsc::Sender<serde_json::Value>,
    subscriptions: Mutex<HashMap<String, SubscriptionEntry>>,
}

impl ConnectionState {
    fn new(outgoing: mpsc::Sender<serde_json::Value>) -> Self {
        Self {
            outgoing,
            subscriptions: Mutex::new(HashMap::new()),
        }
    }

    async fn insert_subscription(&self, id: String, entry: SubscriptionEntry) {
        let mut guard = self.subscriptions.lock().await;
        guard.insert(id, entry);
    }

    async fn remove_subscription(&self, id: &str) -> Option<SubscriptionEntry> {
        let mut guard = self.subscriptions.lock().await;
        guard.remove(id)
    }

    async fn shutdown(&self) {
        let mut guard = self.subscriptions.lock().await;
        for (_, mut entry) in guard.drain() {
            if let Some(cancel) = entry.cancel.take() {
                let _ = cancel.send(());
            }
            entry.task.abort();
        }
    }
}

struct McpService {
    config: McpConfig,
    registry: SessionRegistry,
    leases: Arc<LeaseManager>,
    session_filter: Option<HashSet<String>>,
}

impl McpService {
    fn new(config: McpConfig, registry: SessionRegistry, leases: Arc<LeaseManager>) -> Self {
        let session_filter = config
            .session_filter
            .as_ref()
            .map(|items| items.iter().cloned().collect());
        Self {
            config,
            registry,
            leases,
            session_filter,
        }
    }

    async fn handle_request(
        self: &Arc<Self>,
        state: &Arc<ConnectionState>,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let id = request.id.clone();
        let method = request.method.as_str();
        let params = request.params.unwrap_or_else(|| serde_json::json!({}));
        match method {
            "initialize" => {
                let response_id = id.clone().unwrap_or(serde_json::Value::Null);
                let response = JsonRpcResult::new(
                    response_id,
                    serde_json::json!({
                        "protocolVersion": "2024-10-01",
                        "capabilities": {
                            "resources": true,
                            "tools": true,
                            "notifications": ["resources/updated"]
                        }
                    }),
                );
                Some(JsonRpcResponse::Result(response))
            }
            "ping" => id.map(|value| {
                JsonRpcResponse::Result(JsonRpcResult::new(value, serde_json::json!({"ok": true})))
            }),
            "resources/list" => {
                let result = self.resources_list();
                Some(JsonRpcResponse::Result(JsonRpcResult::new(
                    id.unwrap_or(serde_json::Value::Null),
                    result,
                )))
            }
            "resources/read" => match self.resources_read(&params).await {
                Ok(value) => Some(JsonRpcResponse::Result(JsonRpcResult::new(
                    id.unwrap_or(serde_json::Value::Null),
                    value,
                ))),
                Err(err) => Some(err.into_response(id)),
            },
            "resources/subscribe" => match self.resources_subscribe(state, &params).await {
                Ok(value) => Some(JsonRpcResponse::Result(JsonRpcResult::new(
                    id.unwrap_or(serde_json::Value::Null),
                    value,
                ))),
                Err(err) => Some(err.into_response(id)),
            },
            "resources/unsubscribe" => match self.resources_unsubscribe(state, &params).await {
                Ok(value) => Some(JsonRpcResponse::Result(JsonRpcResult::new(
                    id.unwrap_or(serde_json::Value::Null),
                    value,
                ))),
                Err(err) => Some(err.into_response(id)),
            },
            "tools/list" => {
                let result = self.tools_list();
                Some(JsonRpcResponse::Result(JsonRpcResult::new(
                    id.unwrap_or(serde_json::Value::Null),
                    result,
                )))
            }
            "tools/call" => match self.tools_call(state, &params).await {
                Ok(value) => Some(JsonRpcResponse::Result(JsonRpcResult::new(
                    id.unwrap_or(serde_json::Value::Null),
                    value,
                ))),
                Err(err) => Some(err.into_response(id)),
            },
            _ => Some(method_not_found(id, method)),
        }
    }

    fn resources_list(&self) -> serde_json::Value {
        let sessions = self.registry.list_terminal_sessions();
        let mut resources = Vec::new();
        for session in sessions {
            if self.session_denied(&session) {
                continue;
            }
            let surface = TerminalSurface::new(session);
            resources.extend(surface.describe_resources());
        }
        serde_json::json!({"resources": resources})
    }

    async fn resources_read(
        &self,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, McpError> {
        let target = ResourceTarget::parse(params)?;
        let session = self
            .registry
            .get_terminal(&target.session_id)
            .ok_or_else(|| McpError::not_found("session not found"))?;
        if self.session_denied(&session) {
            return Err(McpError::unauthorized("session not permitted"));
        }
        let surface = TerminalSurface::new(session);
        let value = surface
            .read_resource(&target.resource, target.options.as_ref())
            .map_err(|err| McpError::internal(err.to_string()))?;
        Ok(value)
    }

    async fn resources_subscribe(
        &self,
        state: &Arc<ConnectionState>,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, McpError> {
        let target = ResourceTarget::parse(params)?;
        if target.resource != crate::mcp::terminal::TerminalResource::Grid {
            return Err(McpError::invalid("resource does not support subscription"));
        }
        let session = self
            .registry
            .get_terminal(&target.session_id)
            .ok_or_else(|| McpError::not_found("session not found"))?;
        if self.session_denied(&session) {
            return Err(McpError::unauthorized("session not permitted"));
        }
        let surface = TerminalSurface::new(session);
        let subscription_id = uuid::Uuid::new_v4().to_string();
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let handle = surface
            .start_subscription(
                &target.resource,
                target.options.as_ref(),
                state.outgoing.clone(),
                subscription_id.clone(),
                cancel_rx,
            )
            .map_err(|err| McpError::internal(err.to_string()))?;
        state
            .insert_subscription(
                subscription_id.clone(),
                SubscriptionEntry {
                    cancel: Some(cancel_tx),
                    task: handle,
                },
            )
            .await;
        Ok(serde_json::json!({"subscription_id": subscription_id}))
    }

    async fn resources_unsubscribe(
        &self,
        state: &Arc<ConnectionState>,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, McpError> {
        let subscription_id = params
            .get("subscription_id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| McpError::invalid("subscription_id missing"))?;
        if let Some(mut entry) = state.remove_subscription(subscription_id).await {
            if let Some(cancel) = entry.cancel.take() {
                let _ = cancel.send(());
            }
            entry.task.abort();
            Ok(serde_json::json!({"status": "ok"}))
        } else {
            Err(McpError::not_found("subscription not found"))
        }
    }

    fn tools_list(&self) -> serde_json::Value {
        let read_only = self.config.effective_read_only();
        let sessions = self.registry.list_terminal_sessions();
        let mut seen = HashMap::new();
        for session in sessions {
            if self.session_denied(&session) {
                continue;
            }
            let surface = TerminalSurface::new(session);
            for descriptor in surface.list_tools(read_only) {
                seen.entry(descriptor.name.clone()).or_insert(descriptor);
            }
        }
        serde_json::json!({"tools": seen.into_values().collect::<Vec<_>>()})
    }

    async fn tools_call(
        &self,
        _state: &Arc<ConnectionState>,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, McpError> {
        let name = params
            .get("name")
            .or_else(|| params.get("tool"))
            .and_then(|value| value.as_str())
            .ok_or_else(|| McpError::invalid("tool name missing"))?;
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        match name {
            crate::mcp::terminal::LIST_SESSIONS => {
                crate::mcp::terminal::handle_list_sessions(&self.leases, &arguments)
                    .map_err(|err| McpError::internal(err.to_string()))
            }
            _ => {
                let session_id = arguments
                    .get("session_id")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| McpError::invalid("session_id required"))?;
                let surface = self.resolve_surface(session_id)?;
                surface
                    .call_tool(name, &arguments, &self.leases)
                    .map_err(|err| McpError::internal(err.to_string()))
            }
        }
    }

    fn session_denied(&self, session: &Arc<TerminalSession>) -> bool {
        if let Some(filter) = &self.session_filter {
            !filter.contains(&session.session_id)
        } else {
            false
        }
    }

    fn resolve_surface(&self, session_id: &str) -> Result<TerminalSurface, McpError> {
        let session = self
            .registry
            .get_terminal(session_id)
            .ok_or_else(|| McpError::not_found("session not found"))?;
        if self.session_denied(&session) {
            return Err(McpError::unauthorized("session not permitted"));
        }
        Ok(TerminalSurface::new(session))
    }
}

struct ResourceTarget {
    session_id: String,
    resource: crate::mcp::terminal::TerminalResource,
    options: Option<serde_json::Value>,
}

impl ResourceTarget {
    fn parse(params: &serde_json::Value) -> Result<Self, McpError> {
        let resource = params
            .get("resource")
            .ok_or_else(|| McpError::invalid("resource missing"))?;
        let uri = resource
            .get("uri")
            .and_then(|value| value.as_str())
            .ok_or_else(|| McpError::invalid("resource.uri missing"))?;
        let (session_id, resource_kind) = parse_resource_uri(uri)?;
        let options = params.get("options").cloned();
        Ok(Self {
            session_id,
            resource: resource_kind,
            options,
        })
    }
}

fn parse_resource_uri(
    uri: &str,
) -> Result<(String, crate::mcp::terminal::TerminalResource), McpError> {
    if !uri.starts_with("beach://session/") {
        return Err(McpError::invalid("unsupported resource uri"));
    }
    let suffix = &uri["beach://session/".len()..];
    let mut parts = suffix.split('/');
    let session_id = parts
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| McpError::invalid("missing session id"))?;
    let resource = match parts.collect::<Vec<_>>().as_slice() {
        ["terminal", "grid"] => crate::mcp::terminal::TerminalResource::Grid,
        ["terminal", "history"] => crate::mcp::terminal::TerminalResource::History,
        ["terminal", "cursor"] => crate::mcp::terminal::TerminalResource::Cursor,
        _ => return Err(McpError::invalid("unknown resource")),
    };
    Ok((session_id.to_string(), resource))
}

#[derive(Debug)]
enum McpError {
    Invalid(String),
    NotFound(String),
    Unauthorized(String),
    Internal(String),
}

impl McpError {
    fn invalid(message: impl Into<String>) -> Self {
        McpError::Invalid(message.into())
    }
    fn not_found(message: impl Into<String>) -> Self {
        McpError::NotFound(message.into())
    }
    fn unauthorized(message: impl Into<String>) -> Self {
        McpError::Unauthorized(message.into())
    }
    fn internal(message: impl Into<String>) -> Self {
        McpError::Internal(message.into())
    }

    fn into_response(self, id: Option<serde_json::Value>) -> JsonRpcResponse {
        match self {
            McpError::Invalid(message) => invalid_params(id, message),
            McpError::NotFound(message) => method_not_found(id, &message),
            McpError::Unauthorized(message) => unauthorized(id, message),
            McpError::Internal(message) => internal_error(id, message),
        }
    }
}

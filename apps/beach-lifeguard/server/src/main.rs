use anyhow::{Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Query, State, WebSocketUpgrade,
    },
    http::{header, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use base64::engine::general_purpose::STANDARD_NO_PAD;
use base64::Engine;
use beach_lifeguard_client::CompressionStrategy;
use beach_lifeguard_core::{
    is_telemetry_enabled, FallbackTokenClaims, TelemetryPreference, TokenValidationError,
};
use clap::Parser;
use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use metrics::{counter, gauge, histogram};
use metrics_exporter_prometheus::PrometheusHandle;
use redis::{aio::ConnectionManager, AsyncCommands, RedisResult};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use thiserror::Error;
use time::OffsetDateTime;
use tokio::{signal, time::timeout};
use tracing::{info, warn};
use uuid::Uuid;

mod session;
mod telemetry;

#[derive(Debug, Clone)]
struct ServerConfig {
    listen_addr: SocketAddr,
    redis_url: String,
    disable_oidc: bool,
    shutdown_grace: Duration,
    handshake_timeout: Duration,
}

impl ServerConfig {
    fn oidc_enabled(&self) -> bool {
        !self.disable_oidc
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "beach-lifeguard-server",
    author,
    version,
    about = "Beach WebSocket fallback server (handshake skeleton)"
)]
struct Cli {
    /// Address to bind the websocket listener to.
    #[arg(
        long,
        env = "BEACH_LIFEGUARD_LISTEN_ADDR",
        default_value = "127.0.0.1:9443"
    )]
    listen_addr: String,

    /// Redis connection URI used for guardrail counters and token cache.
    #[arg(
        long,
        env = "BEACH_LIFEGUARD_REDIS_URL",
        default_value = "redis://127.0.0.1:6379"
    )]
    redis_url: String,

    /// Disable OIDC entitlement validation (development mode only).
    #[arg(long, env = "BEACH_LIFEGUARD_DISABLE_OIDC", default_value_t = false)]
    disable_oidc: bool,

    /// Grace period applied during shutdown.
    #[arg(long, env = "BEACH_LIFEGUARD_SHUTDOWN_GRACE_SECS", default_value_t = 5)]
    shutdown_grace_secs: u64,

    /// Maximum time clients have to send their ClientHello frame.
    #[arg(
        long,
        env = "BEACH_LIFEGUARD_HANDSHAKE_TIMEOUT_SECS",
        default_value_t = 5
    )]
    handshake_timeout_secs: u64,
}

impl TryFrom<Cli> for ServerConfig {
    type Error = anyhow::Error;

    fn try_from(cli: Cli) -> Result<Self, Self::Error> {
        let listen_addr: SocketAddr = cli
            .listen_addr
            .parse()
            .with_context(|| format!("invalid listen address: {}", cli.listen_addr))?;
        Ok(ServerConfig {
            listen_addr,
            redis_url: cli.redis_url,
            disable_oidc: cli.disable_oidc,
            shutdown_grace: Duration::from_secs(cli.shutdown_grace_secs),
            handshake_timeout: Duration::from_secs(cli.handshake_timeout_secs),
        })
    }
}

struct AppState {
    redis: ConnectionManager,
    require_oidc: bool,
    handshake_timeout: Duration,
    registry: session::SessionRegistry,
    metrics: PrometheusHandle,
}

#[derive(Debug, Serialize)]
struct StatsResponse {
    active_sessions: usize,
    active_connections: usize,
    total_connections: i64,
    total_messages_forwarded: i64,
    total_bytes_forwarded: i64,
    sessions: Vec<SessionStatsEntry>,
}

#[derive(Debug, Serialize)]
struct SessionStatsEntry {
    session_id: String,
    connections: usize,
}

const METRIC_CONNECTIONS_TOTAL: &str = "fallback:metrics:connections_total";
const METRIC_MESSAGES_TOTAL: &str = "fallback:metrics:messages_forwarded_total";
const METRIC_BYTES_TOTAL: &str = "fallback:metrics:bytes_forwarded_total";

#[derive(Debug, Deserialize)]
struct TokenQuery {
    token: String,
}

#[derive(Debug, Deserialize)]
struct ClientHello {
    session_id: Uuid,
    protocol_version: u16,
    compression: CompressionStrategy,
    telemetry: TelemetryPreference,
}

#[tokio::main]
async fn main() -> Result<()> {
    let telemetry = telemetry::Telemetry::init()?;

    let cli = Cli::parse();
    let config = ServerConfig::try_from(cli)?;
    info!(
        listen_addr = %config.listen_addr,
        redis_url = %config.redis_url,
        oidc_enabled = config.oidc_enabled(),
        "starting beach-lifeguard server"
    );

    run(config, telemetry.metrics_handle()).await
}

async fn run(config: ServerConfig, metrics: PrometheusHandle) -> Result<()> {
    let client =
        redis::Client::open(config.redis_url.clone()).context("failed to create redis client")?;
    let manager = ConnectionManager::new(client)
        .await
        .context("failed to connect to redis")?;
    let registry = session::SessionRegistry::new(session::SessionConfig::default());
    let state = Arc::new(AppState {
        redis: manager,
        require_oidc: config.oidc_enabled(),
        handshake_timeout: config.handshake_timeout,
        registry: registry.clone(),
        metrics,
    });

    let recycler_handle = registry.spawn_recycler();

    let router = Router::new()
        .route("/healthz", get(health_handler))
        .route("/debug/stats", get(stats_handler))
        .route("/metrics", get(metrics_handler))
        .route("/ws", get(ws_handler))
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind(config.listen_addr)
        .await
        .context("failed to bind listener")?;

    info!("beach-lifeguard listening on {}", config.listen_addr);

    let graceful = axum::serve(listener, router).with_graceful_shutdown(shutdown_signal());
    graceful.await.context("server shutdown with error")?;

    info!(
        grace_seconds = config.shutdown_grace.as_secs(),
        "shutdown signal received; sleeping for graceful period"
    );
    recycler_handle.abort();
    tokio::time::sleep(config.shutdown_grace).await;
    info!("graceful shutdown complete");

    Ok(())
}

async fn shutdown_signal() {
    let _ = signal::ctrl_c().await;
}

async fn health_handler() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

async fn stats_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let stats = state.stats().await;
    Json(stats)
}

async fn metrics_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let body = state.render_metrics();
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], body)
}

async fn ws_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TokenQuery>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    match decode_token(&query.token) {
        Ok(claims) => ws
            .on_upgrade(move |socket| handle_connection(socket, state, claims))
            .into_response(),
        Err(err) => {
            record_token_error(&err);
            warn!("token validation failed: {err}");
            StatusCode::UNAUTHORIZED.into_response()
        }
    }
}

fn decode_token(token: &str) -> Result<FallbackTokenClaims, TokenError> {
    let bytes = STANDARD_NO_PAD
        .decode(token)
        .map_err(TokenError::InvalidBase64)?;
    let claims: FallbackTokenClaims =
        serde_json::from_slice(&bytes).map_err(TokenError::InvalidJson)?;
    claims
        .ensure_not_expired(OffsetDateTime::now_utc())
        .map_err(TokenError::Expired)?;
    Ok(claims)
}

async fn handle_connection(socket: WebSocket, state: Arc<AppState>, claims: FallbackTokenClaims) {
    if let Err(err) = upgrade_connection(socket, state, claims).await {
        warn!("connection ended with error: {err:?}");
    }
}

async fn upgrade_connection(
    socket: WebSocket,
    state: Arc<AppState>,
    claims: FallbackTokenClaims,
) -> Result<()> {
    let (mut ws_tx, mut ws_rx) = socket.split();

    let handshake_started = Instant::now();
    let client_hello = match perform_handshake(
        state.handshake_timeout,
        state.require_oidc,
        &claims,
        &mut ws_tx,
        &mut ws_rx,
    )
    .await
    {
        Ok(hello) => {
            record_handshake_success(&hello, handshake_started.elapsed());
            hello
        }
        Err(err) => {
            record_handshake_failure(&err);
            return Err(err.into());
        }
    };

    let connection_id = Uuid::new_v4();
    let registration = state
        .registry
        .register(claims.session_id, connection_id, client_hello.compression)
        .await;
    state
        .on_connection_added(
            claims.session_id,
            registration.active_connections,
            registration.total_sessions,
        )
        .await;
    let mut rx = registration.receiver;

    info!(
        session_id = %client_hello.session_id,
        telemetry_enabled = is_telemetry_enabled(client_hello.telemetry),
        connection_id = %connection_id,
        "fallback connection established"
    );

    let writer_session = claims.session_id;
    let writer_connection = connection_id;
    let writer = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if ws_tx.send(message).await.is_err() {
                break;
            }
        }
        info!(
            session_id = %writer_session,
            connection_id = %writer_connection,
            "writer task finished"
        );
    });

    while let Some(message) = ws_rx.next().await {
        match message {
            Ok(frame) => match frame {
                Message::Close(frame) => {
                    info!(
                        session_id = %claims.session_id,
                        connection_id = %connection_id,
                        reason = ?frame.map(|f| f.reason.to_string()),
                        "client closed websocket"
                    );
                    break;
                }
                Message::Text(_) | Message::Binary(_) => {
                    let metrics = state
                        .registry
                        .broadcast(claims.session_id, connection_id, frame)
                        .await;
                    if metrics.delivered > 0 {
                        state
                            .record_message_forwarded(
                                claims.session_id,
                                metrics.delivered,
                                metrics.bytes,
                            )
                            .await;
                    }
                }
                _ => continue,
            },
            Err(err) => {
                warn!(
                    session_id = %claims.session_id,
                    connection_id = %connection_id,
                    error = %err,
                    "error receiving message"
                );
                break;
            }
        }
    }

    let removal = state
        .registry
        .unregister(claims.session_id, connection_id)
        .await;
    state
        .on_connection_removed(
            claims.session_id,
            removal.active_connections,
            removal.total_sessions,
        )
        .await;
    counter!(
        "beach_lifeguard_connections_closed_total",
        1,
        "session_id" => claims.session_id.to_string()
    );
    writer.abort();

    info!(
        session_id = %claims.session_id,
        connection_id = %connection_id,
        "fallback connection closed"
    );

    Ok(())
}

#[derive(Debug, Error)]
enum TokenError {
    #[error("invalid base64 token: {0}")]
    InvalidBase64(base64::DecodeError),
    #[error("invalid token payload: {0}")]
    InvalidJson(serde_json::Error),
    #[error("token expired")]
    Expired(#[from] TokenValidationError),
}

#[derive(Debug, Error)]
enum HandshakeError {
    #[error("handshake timed out")]
    TimedOut,
    #[error("socket closed during handshake: {0:?}")]
    Closed(Option<String>),
    #[error("client sent invalid payload: {0}")]
    InvalidPayload(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("unexpected frame type")]
    UnexpectedFrame(Message),
    #[error("client closed socket before handshake")]
    SocketClosed,
    #[error("session id mismatch between token and client hello")]
    SessionMismatch,
    #[error("token missing required entitlement")]
    MissingEntitlement,
}

async fn perform_handshake(
    handshake_timeout: Duration,
    require_oidc: bool,
    claims: &FallbackTokenClaims,
    ws_tx: &mut SplitSink<WebSocket, Message>,
    ws_rx: &mut SplitStream<WebSocket>,
) -> Result<ClientHello, HandshakeError> {
    let hello_msg = timeout(handshake_timeout, ws_rx.next())
        .await
        .map_err(|_| HandshakeError::TimedOut)?
        .ok_or(HandshakeError::SocketClosed)?;

    let message = hello_msg.map_err(|err| HandshakeError::Protocol(err.to_string()))?;
    let client_hello = match message {
        Message::Text(text) => serde_json::from_str::<ClientHello>(&text)
            .map_err(|err| HandshakeError::InvalidPayload(err.to_string()))?,
        Message::Binary(bytes) => serde_json::from_slice::<ClientHello>(&bytes)
            .map_err(|err| HandshakeError::InvalidPayload(err.to_string()))?,
        Message::Close(frame) => {
            let reason = frame.map(|f| f.reason.to_string());
            return Err(HandshakeError::Closed(reason));
        }
        other => {
            return Err(HandshakeError::UnexpectedFrame(other));
        }
    };

    if client_hello.session_id != claims.session_id {
        return Err(HandshakeError::SessionMismatch);
    }

    if require_oidc && !claims.feature_bits.fallback_authorized {
        return Err(HandshakeError::MissingEntitlement);
    }

    let server_response = beach_lifeguard_client::ServerHello {
        accepted_compression: client_hello.compression,
        feature_bits: claims.feature_bits,
    };

    let payload = serde_json::to_string(&server_response)
        .map_err(|err| HandshakeError::Protocol(err.to_string()))?;
    ws_tx
        .send(Message::Text(payload))
        .await
        .map_err(|err| HandshakeError::Protocol(err.to_string()))?;

    Ok(client_hello)
}

impl AppState {
    async fn on_connection_added(&self, session_id: Uuid, active: usize, total_sessions: usize) {
        let session_label = session_id.to_string();
        gauge!(
            "beach_lifeguard_connections_active",
            active as f64,
            "session_id" => session_label.clone()
        );
        gauge!("beach_lifeguard_sessions_active", total_sessions as f64);
        counter!(
            "beach_lifeguard_connections_total",
            1,
            "session_id" => session_label
        );

        self.record_connection_added(session_id, active).await;
    }

    async fn on_connection_removed(&self, session_id: Uuid, active: usize, total_sessions: usize) {
        let session_label = session_id.to_string();
        gauge!(
            "beach_lifeguard_connections_active",
            active as f64,
            "session_id" => session_label
        );
        gauge!("beach_lifeguard_sessions_active", total_sessions as f64);

        self.record_connection_state(session_id, active).await;
    }

    async fn record_connection_added(&self, session_id: Uuid, active: usize) {
        let active_key = format!("fallback:session:{}:connections_active", session_id);
        let total_key = "fallback:metrics:connections_total";
        if let Err(err) = self
            .with_redis(|mut conn| async move {
                conn.set::<_, _, ()>(&active_key, active as i64).await?;
                conn.incr::<_, _, i64>(total_key, 1).await.map(|_| ())
            })
            .await
        {
            warn!(error = %err, "failed to record connection add telemetry");
        }
    }

    async fn record_connection_state(&self, session_id: Uuid, active: usize) {
        let active_key = format!("fallback:session:{}:connections_active", session_id);
        if let Err(err) = self
            .with_redis(
                |mut conn| async move { conn.set::<_, _, ()>(&active_key, active as i64).await },
            )
            .await
        {
            warn!(error = %err, "failed to update active connection count");
        }
    }

    async fn record_message_forwarded(&self, session_id: Uuid, delivered: usize, bytes: usize) {
        if delivered == 0 {
            return;
        }

        let session_label = session_id.to_string();
        counter!(
            "beach_lifeguard_messages_forwarded_total",
            delivered as u64,
            "session_id" => session_label.clone()
        );
        if bytes > 0 {
            counter!(
                "beach_lifeguard_bytes_forwarded_total",
                bytes as u64,
                "session_id" => session_label.clone()
            );
            let per_message = bytes as f64 / delivered as f64;
            histogram!(
                "beach_lifeguard_message_size_bytes",
                per_message,
                "session_id" => session_label.clone()
            );
        }

        let count_key = "fallback:metrics:messages_forwarded_total";
        let byte_key = "fallback:metrics:bytes_forwarded_total";
        if let Err(err) = self
            .with_redis(|mut conn| async move {
                conn.incr::<_, _, i64>(count_key, delivered as i64).await?;
                if bytes > 0 {
                    conn.incr::<_, _, i64>(byte_key, bytes as i64).await?;
                }
                let per_session_key = format!("fallback:session:{}:messages_forwarded", session_id);
                conn.incr::<_, _, i64>(&per_session_key, delivered as i64)
                    .await
                    .map(|_| ())
            })
            .await
        {
            warn!(error = %err, "failed to record message telemetry");
        }
    }

    async fn with_redis<F, Fut, T>(&self, f: F) -> RedisResult<T>
    where
        F: FnOnce(ConnectionManager) -> Fut,
        Fut: std::future::Future<Output = RedisResult<T>>,
    {
        let manager = self.redis.clone();
        f(manager).await
    }

    async fn stats(&self) -> StatsResponse {
        let snapshot = self.registry.snapshot().await;
        let active_sessions = snapshot.len();
        let mut sessions = Vec::with_capacity(active_sessions);
        let mut active_connections = 0usize;
        for entry in snapshot {
            active_connections += entry.connections;
            sessions.push(SessionStatsEntry {
                session_id: entry.session_id.to_string(),
                connections: entry.connections,
            });
        }

        let (total_connections, total_messages, total_bytes) = match self
            .with_redis(|mut conn| async move {
                let connections = conn
                    .get::<_, Option<i64>>(METRIC_CONNECTIONS_TOTAL)
                    .await?
                    .unwrap_or(0);
                let messages = conn
                    .get::<_, Option<i64>>(METRIC_MESSAGES_TOTAL)
                    .await?
                    .unwrap_or(0);
                let bytes = conn
                    .get::<_, Option<i64>>(METRIC_BYTES_TOTAL)
                    .await?
                    .unwrap_or(0);
                Ok((connections, messages, bytes))
            })
            .await
        {
            Ok(tuple) => tuple,
            Err(err) => {
                warn!(error = %err, "failed to fetch aggregate metrics from redis");
                (0, 0, 0)
            }
        };

        StatsResponse {
            active_sessions,
            active_connections,
            total_connections,
            total_messages_forwarded: total_messages,
            total_bytes_forwarded: total_bytes,
            sessions,
        }
    }

    fn render_metrics(&self) -> String {
        self.metrics.render()
    }
}

fn record_handshake_success(client_hello: &ClientHello, duration: Duration) {
    let protocol_label = client_hello.protocol_version.to_string();
    counter!(
        "beach_lifeguard_handshakes_success_total",
        1,
        "protocol_version" => protocol_label.clone()
    );
    histogram!(
        "beach_lifeguard_handshake_duration_ms",
        duration.as_secs_f64() * 1000.0,
        "protocol_version" => protocol_label
    );
    if matches!(client_hello.telemetry, TelemetryPreference::Enabled) {
        counter!("beach_lifeguard_telemetry_opt_in_handshakes_total", 1);
    }
}

fn record_handshake_failure(error: &HandshakeError) {
    counter!(
        "beach_lifeguard_handshakes_failure_total",
        1,
        "reason" => error.metric_label()
    );
}

fn record_token_error(error: &TokenError) {
    counter!(
        "beach_lifeguard_token_validation_failure_total",
        1,
        "reason" => error.metric_label()
    );
}

impl HandshakeError {
    fn metric_label(&self) -> &'static str {
        match self {
            HandshakeError::TimedOut => "timeout",
            HandshakeError::Closed(_) => "client_closed",
            HandshakeError::InvalidPayload(_) => "invalid_payload",
            HandshakeError::Protocol(_) => "protocol_error",
            HandshakeError::UnexpectedFrame(_) => "unexpected_frame",
            HandshakeError::SocketClosed => "socket_closed",
            HandshakeError::SessionMismatch => "session_mismatch",
            HandshakeError::MissingEntitlement => "missing_entitlement",
        }
    }
}

impl TokenError {
    fn metric_label(&self) -> &'static str {
        match self {
            TokenError::InvalidBase64(_) => "invalid_base64",
            TokenError::InvalidJson(_) => "invalid_json",
            TokenError::Expired(_) => "token_expired",
        }
    }
}

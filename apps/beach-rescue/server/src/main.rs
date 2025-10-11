use anyhow::{Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Query, State, WebSocketUpgrade,
    },
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use base64::engine::general_purpose::STANDARD_NO_PAD;
use base64::Engine;
use beach_rescue_core::{
    is_telemetry_enabled, FallbackTokenClaims, TelemetryPreference, TokenValidationError,
};
use clap::Parser;
use redis::aio::ConnectionManager;
use serde::Deserialize;
use serde_json::json;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{signal, time::timeout};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

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
    name = "beach-rescue-server",
    author,
    version,
    about = "Beach WebSocket fallback server (handshake skeleton)"
)]
struct Cli {
    /// Address to bind the websocket listener to.
    #[arg(
        long,
        env = "BEACH_RESCUE_LISTEN_ADDR",
        default_value = "127.0.0.1:9443"
    )]
    listen_addr: String,

    /// Redis connection URI used for guardrail counters and token cache.
    #[arg(
        long,
        env = "BEACH_RESCUE_REDIS_URL",
        default_value = "redis://127.0.0.1:6379"
    )]
    redis_url: String,

    /// Disable OIDC entitlement validation (development mode only).
    #[arg(long, env = "BEACH_RESCUE_DISABLE_OIDC", default_value_t = false)]
    disable_oidc: bool,

    /// Grace period applied during shutdown.
    #[arg(long, env = "BEACH_RESCUE_SHUTDOWN_GRACE_SECS", default_value_t = 5)]
    shutdown_grace_secs: u64,

    /// Maximum time clients have to send their ClientHello frame.
    #[arg(long, env = "BEACH_RESCUE_HANDSHAKE_TIMEOUT_SECS", default_value_t = 5)]
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

#[derive(Clone)]
struct AppState {
    _redis: ConnectionManager,
    require_oidc: bool,
    handshake_timeout: Duration,
}

#[derive(Debug, Deserialize)]
struct TokenQuery {
    token: String,
}

#[derive(Debug, Deserialize)]
struct ClientHello {
    session_id: Uuid,
    protocol_version: u16,
    compression: beach_rescue_client::CompressionStrategy,
    telemetry: TelemetryPreference,
}

fn init_tracing() {
    let filter_layer = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter_layer)
        .with_target(false)
        .init();
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let cli = Cli::parse();
    let config = ServerConfig::try_from(cli)?;
    info!(
        listen_addr = %config.listen_addr,
        redis_url = %config.redis_url,
        oidc_enabled = config.oidc_enabled(),
        "starting beach-rescue server"
    );

    run(config).await
}

async fn run(config: ServerConfig) -> Result<()> {
    let client =
        redis::Client::open(config.redis_url.clone()).context("failed to create redis client")?;
    let manager = ConnectionManager::new(client)
        .await
        .context("failed to connect to redis")?;
    let state = AppState {
        _redis: manager,
        require_oidc: config.oidc_enabled(),
        handshake_timeout: config.handshake_timeout,
    };

    let router = Router::new()
        .route("/healthz", get(health_handler))
        .route("/ws", get(ws_handler))
        .with_state(Arc::new(state));

    let listener = tokio::net::TcpListener::bind(config.listen_addr)
        .await
        .context("failed to bind listener")?;

    info!("beach-rescue listening on {}", config.listen_addr);

    let graceful = axum::serve(listener, router).with_graceful_shutdown(shutdown_signal());
    graceful.await.context("server shutdown with error")?;

    info!(
        grace_seconds = config.shutdown_grace.as_secs(),
        "shutdown signal received; sleeping for graceful period"
    );
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
        .ensure_not_expired(time::OffsetDateTime::now_utc())
        .map_err(TokenError::Expired)?;
    Ok(claims)
}

async fn handle_connection(socket: WebSocket, state: Arc<AppState>, claims: FallbackTokenClaims) {
    if let Err(err) = upgrade_connection(socket, state, claims).await {
        warn!("connection ended with error: {err:?}");
    }
}

async fn upgrade_connection(
    mut socket: WebSocket,
    state: Arc<AppState>,
    claims: FallbackTokenClaims,
) -> Result<()> {
    let hello_msg = timeout(state.handshake_timeout, socket.recv())
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
            return Err(HandshakeError::Closed(frame.map(|f| f.reason)).into());
        }
        other => {
            return Err(HandshakeError::UnexpectedFrame(other).into());
        }
    };

    if client_hello.session_id != claims.session_id {
        return Err(HandshakeError::SessionMismatch.into());
    }

    if state.require_oidc && !claims.feature_bits.telemetry_enabled {
        return Err(HandshakeError::MissingEntitlement.into());
    }

    let server_response = beach_rescue_client::ServerHello {
        accepted_compression: client_hello.compression,
        feature_bits: claims.feature_bits,
    };

    let payload = serde_json::to_string(&server_response)
        .map_err(|err| HandshakeError::Protocol(err.to_string()))?;
    socket
        .send(Message::Text(payload))
        .await
        .map_err(|err| HandshakeError::Protocol(err.to_string()))?;

    info!(
        session_id = %client_hello.session_id,
        telemetry_enabled = is_telemetry_enabled(client_hello.telemetry),
        "fallback connection established"
    );

    // For now, simply park the connection until the peer disconnects.
    while let Some(message) = socket.recv().await {
        match message {
            Ok(Message::Close(_)) => break,
            Ok(_) => continue,
            Err(err) => {
                return Err(HandshakeError::Protocol(err.to_string()).into());
            }
        }
    }

    info!(
        session_id = %claims.session_id,
        "fallback connection closed"
    );

    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum TokenError {
    #[error("invalid base64 token: {0}")]
    InvalidBase64(base64::DecodeError),
    #[error("invalid token payload: {0}")]
    InvalidJson(serde_json::Error),
    #[error("token expired")]
    Expired(#[from] TokenValidationError),
}

#[derive(Debug, thiserror::Error)]
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

impl From<HandshakeError> for anyhow::Error {
    fn from(value: HandshakeError) -> Self {
        anyhow::anyhow!(value)
    }
}

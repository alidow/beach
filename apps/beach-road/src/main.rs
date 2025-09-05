mod cli;
mod config;
mod handlers;
mod session;
mod signaling;
mod storage;
mod websocket;

use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{info, error};
use tracing_subscriber;

use crate::{
    cli::{Cli, Commands},
    config::Config,
    handlers::{get_session_status, health_check, join_session, register_session, SharedStorage},
    storage::Storage,
    websocket::{websocket_handler, SignalingState},
};
use clap::Parser;

#[tokio::main]
async fn main() {
    // Initialize tracing with environment-based configuration
    // Default to WARN level if RUST_LOG is not set
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "warn");
    }
    tracing_subscriber::fmt::init();

    // Parse CLI arguments
    let cli = Cli::parse();
    
    // Check if running as debug client
    if let Some(Commands::Debug { url, session, command }) = cli.command {
        if let Err(e) = cli::run_debug_client(url, session, command).await {
            error!("Debug client error: {}", e);
            std::process::exit(1);
        }
        return;
    }
    
    // Otherwise, run as server
    // Load configuration
    let config = Config::from_env();
    info!("Starting Beach Road session server on port {}", config.port);
    info!("Redis URL: {}", config.redis_url);
    info!("Session TTL: {} seconds", config.session_ttl_seconds);

    // Initialize Redis storage
    let storage = match Storage::new(&config.redis_url, config.session_ttl_seconds).await {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to connect to Redis: {}", e);
            std::process::exit(1);
        }
    };

    let shared_storage: SharedStorage = Arc::new(Mutex::new(storage));

    // Initialize WebSocket signaling state
    let signaling_state = SignalingState::new(shared_storage.clone());

    // Build the Axum router - split into two parts with different states
    let http_routes = Router::new()
        .route("/health", get(health_check))
        .route("/sessions", post(register_session))
        .route("/sessions/:id", get(get_session_status))
        .route("/sessions/:id/join", post(join_session))
        .with_state(shared_storage);
    
    let ws_routes = Router::new()
        .route("/ws/:session_id", get(websocket_handler))
        .with_state(signaling_state);
    
    let app = Router::new()
        .merge(http_routes)
        .merge(ws_routes)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    // Create the listener
    let addr = format!("0.0.0.0:{}", config.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("Failed to bind to address");

    info!("Beach Road listening on {}", addr);

    // Start the server
    axum::serve(listener, app)
        .await
        .expect("Failed to start server");
}
mod cli;
mod config;
mod entitlement;
mod handlers;
mod session;
mod signaling;
mod storage;
mod viewer_token;
mod websocket;

use axum::{
    routing::{get, post},
    Extension, Router,
};
use base64::engine::general_purpose::{
    STANDARD as BASE64_STANDARD, URL_SAFE_NO_PAD as BASE64_URL_SAFE,
};
use base64::Engine;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{error, info};
use tracing_subscriber;

use crate::{
    cli::{Cli, Commands},
    config::Config,
    entitlement::EntitlementVerifier,
    handlers::{
        get_session_status, get_webrtc_answer, get_webrtc_offer, health_check,
        issue_fallback_token, join_session, metrics_handler, post_webrtc_answer, post_webrtc_offer,
        register_session, FallbackContext, SharedStorage,
    },
    storage::Storage,
    viewer_token::ViewerTokenVerifier,
    websocket::{websocket_handler, SignalingState},
};
use clap::Parser;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

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
    if let Some(Commands::Debug {
        url,
        session,
        command,
    }) = cli.command
    {
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
    info!(
        "Fallback guardrail threshold: {:.3}% (token ttl {} seconds, oidc required: {})",
        config.fallback_guardrail_threshold * 100.0,
        config.fallback_token_ttl_seconds,
        config.fallback_require_oidc
    );
    info!(
        "Fallback token minting paused: {}",
        if config.fallback_paused { "yes" } else { "no" }
    );

    let prometheus_handle = install_metrics_recorder();

    // Initialize Redis storage
    let storage = match Storage::new(&config.redis_url, config.session_ttl_seconds).await {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to connect to Redis: {}", e);
            std::process::exit(1);
        }
    };

    let shared_storage: SharedStorage = Arc::new(storage);

    // Initialize WebSocket signaling state
    let viewer_token_verifier = config.viewer_token_mac_secret.clone().and_then(|secret| {
        let jwks_url = config.fallback_jwks_url.clone()?;
        let issuer = config.fallback_jwt_issuer.clone();
        let audience = config.viewer_token_audience.clone();
        let cache_ttl = Duration::from_secs(config.viewer_token_jwks_cache_ttl_seconds);
        Some(ViewerTokenVerifier::new(
            jwks_url,
            issuer,
            audience,
            cache_ttl,
            decode_viewer_secret(&secret),
        ))
    });

    if viewer_token_verifier.is_some() {
        info!("viewer token verification enabled");
    } else {
        info!("viewer token verification disabled");
    }

    let signaling_state =
        SignalingState::new(shared_storage.clone(), viewer_token_verifier.clone());

    if config.fallback_require_oidc && config.fallback_jwks_url.is_none() {
        error!(
            "FALLBACK_REQUIRE_OIDC=1 but BEACH_GATE_JWKS_URL is not set; fallback token minting will fail"
        );
    }

    let entitlement_verifier = config.fallback_jwks_url.clone().map(|url| {
        let cache_ttl = Duration::from_secs(config.fallback_jwks_cache_ttl_seconds);
        EntitlementVerifier::new(
            url,
            config.fallback_jwt_issuer.clone(),
            config.fallback_jwt_audience.clone(),
            config.fallback_required_entitlement.clone(),
            cache_ttl,
        )
    });

    if let Some(verifier) = entitlement_verifier.as_ref() {
        info!(
            required_entitlement = verifier.required_entitlement(),
            "fallback entitlement verification enabled"
        );
    } else if !config.fallback_require_oidc {
        info!("fallback entitlement verification disabled (proof optional)");
    }

    let fallback_state = FallbackContext {
        storage: shared_storage.clone(),
        guardrail_threshold: config.fallback_guardrail_threshold,
        token_ttl_seconds: config.fallback_token_ttl_seconds,
        require_oidc: config.fallback_require_oidc,
        paused: config.fallback_paused,
        entitlements: entitlement_verifier,
    };

    // Build the Axum router - split into two parts with different states
    let http_routes = Router::new()
        .route("/health", get(health_check))
        .route("/sessions", post(register_session))
        .route("/sessions/:id", get(get_session_status))
        .route("/sessions/:id/join", post(join_session))
        .route("/sessions/:id/verify-code", post(handlers::verify_code))
        .route("/me/sessions", get(handlers::list_my_sessions))
        .route(
            "/sessions/:id/webrtc/offer",
            get(get_webrtc_offer).post(post_webrtc_offer),
        )
        .route(
            "/sessions/:id/webrtc/answer",
            get(get_webrtc_answer).post(post_webrtc_answer),
        )
        .with_state(shared_storage.clone())
        .layer(Extension(signaling_state.clone()))
        .layer(Extension(viewer_token_verifier.clone()));

    let fallback_routes = Router::new()
        .route("/fallback/token", post(issue_fallback_token))
        .with_state(fallback_state);

    let ws_routes = Router::new()
        .route("/ws/:session_id", get(websocket_handler))
        .with_state(signaling_state);

    let metrics_routes = Router::new()
        .route("/metrics", get(metrics_handler))
        .with_state(prometheus_handle.clone());

    let app = Router::new()
        .merge(http_routes)
        .merge(fallback_routes)
        .merge(ws_routes)
        .merge(metrics_routes)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    // Create the listener
    let addr = format!("0.0.0.0:{}", config.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("Failed to bind to address");

    info!("Beach Road listening on {}", addr);

    // Always print to stdout so users know the server is ready
    println!("🏖️  Beach Road listening on {}", addr);

    // Start the server
    let service = app.into_make_service_with_connect_info::<SocketAddr>();
    axum::serve(listener, service)
        .await
        .expect("Failed to start server");
}

fn decode_viewer_secret(secret: &str) -> Vec<u8> {
    if let Ok(decoded) = BASE64_STANDARD.decode(secret) {
        if !decoded.is_empty() {
            return decoded;
        }
    }
    if let Ok(decoded) = BASE64_URL_SAFE.decode(secret) {
        if !decoded.is_empty() {
            return decoded;
        }
    }
    secret.as_bytes().to_vec()
}

fn install_metrics_recorder() -> PrometheusHandle {
    PrometheusBuilder::new()
        .install_recorder()
        .expect("failed to install Prometheus recorder")
}

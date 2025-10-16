use axum::{
    extract::State,
    routing::{get, Router},
    Json,
};
use serde::Serialize;
use std::{net::SocketAddr, sync::Arc};
use tracing::{info, Level};

#[derive(Clone)]
struct AppState {
    build: BuildInfo,
}

#[derive(Clone, Serialize)]
struct BuildInfo {
    name: &'static str,
    version: &'static str,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let state = AppState {
        build: BuildInfo {
            name: env!("CARGO_PKG_NAME"),
            version: env!("CARGO_PKG_VERSION"),
        },
    };

    let app = Router::new()
        .route("/healthz", get(health_check))
        .with_state(Arc::new(state));

    let addr: SocketAddr = "0.0.0.0:8080".parse()?;
    info!("Starting Beach Manager on {addr}");
    axum::serve(
        tokio::net::TcpListener::bind(addr).await?,
        app.into_make_service(),
    )
    .await?;

    Ok(())
}

async fn health_check(State(state): State<Arc<AppState>>) -> Json<&BuildInfo> {
    Json(&state.build)
}

fn init_tracing() {
    tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
}

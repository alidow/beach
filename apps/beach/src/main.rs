#![recursion_limit = "1024"]

use beach_client_core::telemetry::logging as logctl;
use beach_client_core::terminal::app;
use beach_client_core::terminal::cli;
use beach_client_core::terminal::error::CliError;
use tracing::debug;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run().await {
        eprintln!("❌ {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), CliError> {
    let cli = cli::parse();
    let log_config = cli.logging.to_config();
    logctl::init(&log_config).map_err(|err| CliError::Logging(err.to_string()))?;
    debug!(log_level = ?log_config.level, log_file = ?log_config.file, "logging configured");
    app::run(cli).await
}

#![recursion_limit = "1024"]

use beach_human::telemetry::logging as logctl;
use beach_human::terminal::app;
use beach_human::terminal::cli;
use beach_human::terminal::error::CliError;
use tracing::debug;

#[tokio::main]
async fn main() {
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

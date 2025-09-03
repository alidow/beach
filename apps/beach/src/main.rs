mod client;
mod config;
mod server;
mod session;
mod transport;

use clap::Parser;
use client::{Client, TerminalClient};
use config::Config;
use server::{Server, TerminalServer};
use session::Session;
use transport::webrtc::WebRTCTransport;

#[derive(Parser, Debug)]
#[command(name = "beach")]
struct Cli {
    #[arg(long, short = 'j')]
    join: Option<String>,

    #[arg(long, short = 'p')]
    passphrase: Option<String>,

    // everything after `--`
    #[arg(trailing_var_arg = true)]
    cmd: Vec<String>,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let config = Config::from_env();

    if let Some(session) = cli.join.as_deref() {
        // CLIENT MODE: --join provided; disallow a trailing command
        if !cli.cmd.is_empty() {
            eprintln!("Command after `--` is not allowed when running as a client.");
            std::process::exit(2);
        }

        // Join the session
        match Session::join(session, WebRTCTransport::new(), cli.passphrase).await {
            Ok(client_session) => {
                let client = TerminalClient::new(client_session);
                client.start().await;
            }
            Err(e) => {
                eprintln!("❌ {}", e);
                std::process::exit(1);
            }
        }
    } else {
        // SERVER MODE
        eprintln!("🏖️  Beach Server: Using session server: {}", config.session_server);
        
        // Create the session
        match Session::create(&config, WebRTCTransport::new(), cli.passphrase, cli.cmd).await {
            Ok(server_session) => {
                let server = TerminalServer::new(server_session);
                server.clone().setup_handlers().await;
                server.start().await;
            }
            Err(e) => {
                eprintln!("❌ Failed to create session: {}", e);
                std::process::exit(1);
            }
        }
    }
}

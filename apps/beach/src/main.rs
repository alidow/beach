mod client;
mod config;
pub mod protocol;
pub mod server;
pub mod session;
pub mod subscription;
pub mod transport;

use clap::Parser;
use config::Config;
use server::{Server, TerminalServer};
use session::Session;
use transport::mock::MockTransport;

#[derive(Parser, Debug)]
#[command(name = "beach")]
struct Cli {
    #[arg(long, short = 'j')]
    join: Option<String>,

    #[arg(long, short = 'p')]
    passphrase: Option<String>,

    #[arg(long, help = "Record all PTY I/O to a file for debugging")]
    debug_recorder: Option<String>,

    #[arg(long, help = "Write debug logs to a file")]
    debug_log: Option<String>,

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

        // Join the session (but don't connect WebSocket yet)
        match Session::join(session, MockTransport::new(), cli.passphrase).await {
            Ok((mut client_session, server_addr, session_id)) => {
                // TODO: Create client and set handler
                // let client = TerminalClient::new(client_session.clone());
                // client_session.set_handler(client.clone());
                
                // Now connect WebSocket with handler set
                if let Err(e) = client_session.connect_signaling(&server_addr, &session_id).await {
                    eprintln!("⚠️  Failed to establish WebSocket connection: {}", e);
                    eprintln!("⚠️  Some features may not work without WebSocket connection");
                }
                
                // TODO: Start the client
                // client.start().await;
                eprintln!("Client mode not yet fully implemented");
            }
            Err(e) => {
                eprintln!("❌ {}", e);
                std::process::exit(1);
            }
        }
    } else {
        // SERVER MODE
        // Simply create the terminal server with everything configured
        match TerminalServer::create(
            &config,
            MockTransport::new(),
            cli.passphrase,
            cli.cmd.clone(),
            cli.debug_recorder,
            cli.debug_log,
        ).await {
            Ok(server) => {
                server.start().await;
            }
            Err(e) => {
                eprintln!("❌ Failed to create session: {}", e);
                std::process::exit(1);
            }
        }
    }
}

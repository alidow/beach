mod client;
mod config;
mod protocol;
mod server;
mod session;
mod transport;

use clap::Parser;
use config::Config;
use server::{Server, TerminalServer};
use session::{Session, ServerSession, ClientSession};
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

        // Join the session
        match Session::join(session, MockTransport::new(), cli.passphrase).await {
            Ok(client_session) => {
                // TODO: Implement client
                // let client = TerminalClient::new(client_session);
                // client.start().await;
                eprintln!("Client mode not yet implemented");
            }
            Err(e) => {
                eprintln!("❌ {}", e);
                std::process::exit(1);
            }
        }
    } else {
        // SERVER MODE
        // Using session server silently
        
        // Create the session
        match Session::create(&config, MockTransport::new(), cli.passphrase, cli.cmd.clone()).await {
            Ok(server_session) => {
                let server = TerminalServer::new_with_debug(
                    server_session,
                    cli.debug_recorder,
                    cli.debug_log
                );
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

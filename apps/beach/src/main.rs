mod client;
mod config;
mod debug_log;
pub mod protocol;
pub mod server;
pub mod session;
pub mod subscription;
pub mod transport;

use clap::Parser;
use config::Config;
use client::terminal_client::TerminalClient;
use debug_log::DebugLogger;
use server::{Server, TerminalServer};
use transport::{TransportMode, webrtc::{WebRTCTransport, config::{WebRTCConfig, WebRTCConfigBuilder}}};

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

    #[arg(long, short = 'v', help = "Enable verbose logging for diagnostics")]
    verbose: bool,

    #[arg(long, help = "Don't wait for clients before executing command", default_value = "false")]
    no_wait: bool,

    #[arg(long, help = "Don't wait for WebRTC connection (allow WebSocket fallback)", default_value = "false")]
    no_wait_webrtc: bool,

    #[arg(long, help = "Exit immediately after command finishes", default_value = "false")]
    exit_on_done: bool,

    // everything after `--`
    #[arg(trailing_var_arg = true)]
    cmd: Vec<String>,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let config = Config::from_env();

    // Set the global debug log path if provided
    session::set_debug_log_path(cli.debug_log.clone());

    // Set verbose mode if flag is provided
    if cli.verbose {
        unsafe {
            std::env::set_var("BEACH_VERBOSE", "1");
        }
        // eprintln!("🔍 [VERBOSE] Verbose logging enabled");
    }

    if let Some(session) = cli.join.as_deref() {
        // CLIENT MODE: --join provided; disallow a trailing command
        if !cli.cmd.is_empty() {
            eprintln!("Command after `--` is not allowed when running as a client.");
            std::process::exit(2);
        }

        // Prompt for passphrase if not provided
        let passphrase = if cli.passphrase.is_none() {
            match TerminalClient::<WebRTCTransport>::prompt_passphrase().await {
                Ok(p) => Some(p),
                Err(e) => {
                    eprintln!("❌ {}", e);
                    std::process::exit(1);
                }
            }
        } else {
            cli.passphrase
        };
        
        // Create debug logger if needed
        let debug_logger = cli.debug_log.as_ref().map(|path| {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok();
            DebugLogger::new(file)
        });
        
        // Create WebRTC config with debug logger
        let webrtc_config = WebRTCConfigBuilder::new()
            .mode(TransportMode::Client)
            .debug_logger(debug_logger)
            .build()
            .unwrap();
        
        // Simply create the terminal client with everything configured
        match TerminalClient::create(
            &config,
            WebRTCTransport::new(webrtc_config).await.unwrap(),
            session,
            passphrase,
            cli.debug_log,
        ).await {
            Ok(client) => {
                client.start().await.unwrap_or_else(|e| {
                    eprintln!("❌ Client error: {}", e);
                    std::process::exit(1);
                });
            }
            Err(e) => {
                eprintln!("❌ Failed to join session: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        // SERVER MODE
        // Generate passphrase if not provided
        let passphrase = cli.passphrase.or_else(|| {
            if let Ok(p) = std::env::var("BEACH_PASSPHRASE") {
                Some(p)
            } else {
                // Generate a simple passphrase
                let words = ["beach", "ocean", "wave", "surf", "sand", "shell", "coral", "tide"];
                let mut rng = rand::thread_rng();
                use rand::seq::SliceRandom;
                let word1 = words.choose(&mut rng).unwrap();
                let word2 = words.choose(&mut rng).unwrap();
                let num: u16 = rand::Rng::gen_range(&mut rng, 100..999);
                let generated = format!("{}-{}-{}", word1, word2, num);
                println!("🔑 Passphrase: {}", generated);
                Some(generated)
            }
        });
        
        // Create debug logger if needed
        let debug_logger = cli.debug_log.as_ref().map(|path| {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok();
            DebugLogger::new(file)
        });
        
        // Create WebRTC config with debug logger
        let webrtc_config = WebRTCConfigBuilder::new()
            .mode(TransportMode::Server)
            .debug_logger(debug_logger)
            .build()
            .unwrap();
        
        // Simply create the terminal server with everything configured
        match TerminalServer::create(
            &config,
            WebRTCTransport::new(webrtc_config).await.unwrap(),
            passphrase,
            cli.cmd.clone(),
            cli.debug_recorder,
            cli.debug_log,
        ).await {
            Ok(server) => {
                // Default behavior: wait for client, wait for WebRTC, keep alive
                // Only skip if explicitly requested with --no-wait flags
                let wait_for_client = !cli.no_wait;
                let wait_for_webrtc = !cli.no_wait_webrtc;
                let keep_alive = !cli.exit_on_done;
                server.start_with_wait(wait_for_client, wait_for_webrtc, keep_alive).await;
            }
            Err(e) => {
                eprintln!("❌ Failed to create session: {}", e);
                std::process::exit(1);
            }
        }
    }
}

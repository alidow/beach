use anyhow::Result;
use clap::{Parser, Subcommand};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{info, error};
use tokio::time::{timeout, Duration};

use crate::signaling::{ClientMessage, ServerMessage, DebugRequest, DebugResponse, generate_peer_id, TransportType};

#[derive(Parser, Debug)]
#[command(name = "beach-road")]
#[command(about = "Beach Road session server and debug client")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
    
    /// Run as server (default behavior if no command specified)
    #[arg(long)]
    pub server: bool,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Send a debug signal to a session
    Debug {
        /// Session server URL (e.g., ws://localhost:8080)
        #[arg(short, long, default_value = "ws://localhost:8080")]
        url: String,
        
        /// Session ID to connect to
        #[arg(short, long)]
        session: String,
        
        /// Debug command
        #[command(subcommand)]
        command: DebugCommands,
    },
}

#[derive(Subcommand, Debug)]
pub enum DebugCommands {
    /// Get the current grid view
    GridView {
        /// Width to rewrap to (optional)
        #[arg(short, long)]
        width: Option<u16>,
        
        /// Height to rewrap to (optional)
        #[arg(short = 'e', long)]
        height: Option<u16>,
        
        /// Start from line number (optional)
        #[arg(short = 'l', long)]
        from_line: Option<u64>,
        
        /// Use ANSI colors in output
        #[arg(short, long)]
        ansi: bool,
    },
    
    /// Get terminal statistics
    Stats,
    
    /// Clear terminal history
    Clear,
}

pub async fn run_debug_client(url: String, session: String, command: DebugCommands) -> Result<()> {
    info!("Connecting to {} for session {}", url, session);
    
    // Build WebSocket URL
    let ws_url = format!("{}/ws/{}", url, session);
    
    // Connect to the WebSocket with timeout
    let (ws_stream, _) = match timeout(Duration::from_secs(5), connect_async(&ws_url)).await {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => {
            error!("Failed to connect to {}: {}", ws_url, e);
            return Err(anyhow::anyhow!("Connection failed: {}", e));
        }
        Err(_) => {
            error!("Connection timeout after 5 seconds");
            return Err(anyhow::anyhow!("Connection timeout - is the session server running?"));
        }
    };
    let (mut write, mut read) = ws_stream.split();
    
    // Generate a peer ID
    let peer_id = generate_peer_id();
    
    // Send join message
    let join_msg = ClientMessage::Join {
        peer_id: peer_id.clone(),
        passphrase: None,
        supported_transports: vec![TransportType::Direct],
        preferred_transport: Some(TransportType::Direct),
    };
    
    let join_text = serde_json::to_string(&join_msg)?;
    write.send(Message::Text(join_text.into())).await?;
    
    // Wait for join response with timeout
    let join_timeout = timeout(Duration::from_secs(5), async {
        while let Some(msg) = read.next().await {
            match msg? {
                Message::Text(text) => {
                    let server_msg: ServerMessage = serde_json::from_str(&text)?;
                    match server_msg {
                        ServerMessage::JoinSuccess { .. } => {
                            info!("Successfully joined session");
                            return Ok::<_, anyhow::Error>(());
                        }
                        ServerMessage::JoinError { reason } => {
                            error!("Failed to join session: {}", reason);
                            return Err(anyhow::anyhow!("Join failed: {}", reason));
                        }
                        _ => {
                            // Ignore other messages during join
                        }
                    }
                }
                _ => {}
            }
        }
        Err(anyhow::anyhow!("Connection closed unexpectedly"))
    }).await;
    
    match join_timeout {
        Ok(Ok(())) => {
            // Successfully joined
        }
        Ok(Err(e)) => {
            return Err(e);
        }
        Err(_) => {
            error!("Timeout waiting for join response - session may not exist");
            return Err(anyhow::anyhow!("Session not found or not responding"));
        }
    }
    
    // Send debug request based on command
    let debug_request = match command {
        DebugCommands::GridView { width, height, from_line, .. } => {
            DebugRequest::GetGridView {
                width,
                height,
                at_time: None,
                from_line,
            }
        }
        DebugCommands::Stats => DebugRequest::GetStats,
        DebugCommands::Clear => DebugRequest::ClearHistory,
    };
    
    let debug_msg = ClientMessage::Debug {
        request: debug_request,
    };
    
    let debug_text = serde_json::to_string(&debug_msg)?;
    write.send(Message::Text(debug_text.into())).await?;
    
    // Wait for debug response with timeout
    let debug_timeout = timeout(Duration::from_secs(10), async {
        while let Some(msg) = read.next().await {
            match msg? {
                Message::Text(text) => {
                    let server_msg: ServerMessage = serde_json::from_str(&text)?;
                    if let ServerMessage::Debug { response } = server_msg {
                        return Ok::<_, anyhow::Error>(response);
                    }
                }
                _ => {}
            }
        }
        Err(anyhow::anyhow!("Connection closed before receiving debug response"))
    }).await;
    
    match debug_timeout {
        Ok(Ok(response)) => {
            handle_debug_response(response, matches!(command, DebugCommands::GridView { ansi: true, .. }))?;
        }
        Ok(Err(e)) => {
            return Err(e);
        }
        Err(_) => {
            error!("Timeout waiting for debug response after 10 seconds");
            return Err(anyhow::anyhow!("Debug request timeout - the terminal may be busy or unresponsive"));
        }
    }
    
    // Close connection
    write.send(Message::Close(None)).await?;
    
    Ok(())
}

fn handle_debug_response(response: DebugResponse, use_ansi: bool) -> Result<()> {
    match response {
        DebugResponse::GridView {
            width,
            height,
            cursor_row,
            cursor_col,
            cursor_visible,
            rows,
            ansi_rows,
            timestamp,
            start_line,
            end_line,
        } => {
            println!("╔══════════════════════════════════════════════════════════════╗");
            println!("║                      TERMINAL GRID VIEW                      ║");
            println!("╠══════════════════════════════════════════════════════════════╣");
            println!("║ Dimensions: {}x{}", width, height);
            println!("║ Cursor: ({}, {}) {}", 
                cursor_row, cursor_col,
                if cursor_visible { "visible" } else { "hidden" }
            );
            println!("║ Lines: {} to {}", start_line, end_line);
            println!("║ Timestamp: {}", timestamp);
            println!("╠══════════════════════════════════════════════════════════════╣");
            
            // Print the grid content
            if use_ansi && ansi_rows.is_some() {
                // Use ANSI-colored version
                for row in ansi_rows.unwrap() {
                    println!("║{}║", row);
                }
            } else {
                // Use plain text version
                for row in rows {
                    println!("║{}║", row);
                }
            }
            
            println!("╚══════════════════════════════════════════════════════════════╝");
        }
        
        DebugResponse::Stats {
            history_size_bytes,
            total_deltas,
            total_snapshots,
            current_dimensions,
            session_duration_secs,
        } => {
            println!("╔══════════════════════════════════════════════════════════════╗");
            println!("║                    TERMINAL STATISTICS                       ║");
            println!("╠══════════════════════════════════════════════════════════════╣");
            println!("║ History Size: {} bytes", history_size_bytes);
            println!("║ Total Deltas: {}", total_deltas);
            println!("║ Total Snapshots: {}", total_snapshots);
            println!("║ Current Dimensions: {}x{}", current_dimensions.0, current_dimensions.1);
            println!("║ Session Duration: {} seconds", session_duration_secs);
            println!("╚══════════════════════════════════════════════════════════════╝");
        }
        
        DebugResponse::Success { message } => {
            println!("✅ Success: {}", message);
        }
        
        DebugResponse::Error { message } => {
            eprintln!("❌ Error: {}", message);
        }
    }
    
    Ok(())
}
use anyhow::Result;
use clap::{Parser, Subcommand};
use futures_util::{SinkExt, StreamExt};
use std::io::IsTerminal;
use tokio::time::{timeout, Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error};

use crate::signaling::{
    generate_peer_id, ClientMessage, DebugRequest, DebugResponse, PeerRole, ServerMessage,
    TransportType,
};

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
        /// Number of rows to display (optional)
        #[arg(short = 'n', long)]
        height: Option<u16>,

        /// Start from line number (optional)
        #[arg(short = 'l', long)]
        from_line: Option<u64>,

        /// Get grid from N seconds ago (optional)
        #[arg(long)]
        ago: Option<u64>,
    },

    /// Get terminal statistics
    Stats,

    /// Clear terminal history
    Clear,
}

pub async fn run_debug_client(url: String, session: String, command: DebugCommands) -> Result<()> {
    debug!("Connecting to {} for session {}", url, session);

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
            return Err(anyhow::anyhow!(
                "Connection timeout - is the session server running?"
            ));
        }
    };
    let (mut write, mut read) = ws_stream.split();

    // Generate a peer ID
    let peer_id = generate_peer_id();

    // Send join message
    let join_msg = ClientMessage::Join {
        peer_id: peer_id.clone(),
        passphrase: None,
        viewer_token: None,
        supported_transports: vec![TransportType::Direct],
        preferred_transport: Some(TransportType::Direct),
        label: None,
        mcp: false,
        metadata: None,
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
                        ServerMessage::JoinSuccess { peers, .. } => {
                            // Find the server peer ID
                            let mut found_server_id = String::new();
                            for peer in peers {
                                if peer.role == PeerRole::Server {
                                    found_server_id = peer.id.clone();
                                    break;
                                }
                            }
                            if found_server_id.is_empty() {
                                return Err(anyhow::anyhow!("No server found in session"));
                            }
                            debug!(
                                "Successfully joined session, server peer: {}",
                                found_server_id
                            );
                            return Ok::<_, anyhow::Error>(found_server_id);
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
    })
    .await;

    let server_peer_id = match join_timeout {
        Ok(Ok(peer_id)) => peer_id,
        Ok(Err(e)) => {
            return Err(e);
        }
        Err(_) => {
            error!("Timeout waiting for join response - session may not exist");
            return Err(anyhow::anyhow!("Session not found or not responding"));
        }
    };

    // Send debug request based on command
    let debug_request = match command {
        DebugCommands::GridView {
            height,
            from_line,
            ago,
        } => {
            // Calculate at_time if --ago is specified
            let at_time =
                ago.map(|seconds| chrono::Utc::now() - chrono::Duration::seconds(seconds as i64));

            DebugRequest::GetGridView {
                height,
                at_time,
                from_line,
            }
        }
        DebugCommands::Stats => DebugRequest::GetStats,
        DebugCommands::Clear => DebugRequest::ClearHistory,
    };

    // Wrap debug request in a Custom transport signal to the server
    // Beach expects signals as serde_json::Value, not TransportSignal enum
    let debug_signal = serde_json::json!({
        "transport": "custom",
        "transport_name": "debug",
        "signal_type": "debug_request",
        "payload": {
            "request": debug_request,
        }
    });

    let signal_msg = ClientMessage::Signal {
        to_peer: server_peer_id.clone(),
        signal: debug_signal,
    };

    let signal_text = serde_json::to_string(&signal_msg)?;
    write.send(Message::Text(signal_text.into())).await?;

    // Wait for debug response (or explicit error) with timeout
    let debug_timeout = timeout(Duration::from_secs(10), async {
        while let Some(msg) = read.next().await {
            match msg? {
                Message::Text(text) => {
                    let server_msg: ServerMessage = serde_json::from_str(&text)?;
                    match server_msg {
                        ServerMessage::Signal {
                            from_peer: _,
                            signal,
                        } => {
                            // Check if this is a debug response wrapped in Custom transport (now as serde_json::Value)
                            if let Some(transport) =
                                signal.get("transport").and_then(|v| v.as_str())
                            {
                                if transport == "custom" {
                                    if let (Some(transport_name), Some(signal_type)) = (
                                        signal.get("transport_name").and_then(|v| v.as_str()),
                                        signal.get("signal_type").and_then(|v| v.as_str()),
                                    ) {
                                        if transport_name == "debug"
                                            && signal_type == "debug_response"
                                        {
                                            if let Some(payload) = signal.get("payload") {
                                                if let Some(response) = payload.get("response") {
                                                    if !response.is_null() {
                                                        let debug_response: DebugResponse =
                                                            serde_json::from_value(
                                                                response.clone(),
                                                            )?;
                                                        return Ok::<_, anyhow::Error>(
                                                            debug_response,
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        ServerMessage::Error { message } => {
                            return Err(anyhow::anyhow!("Server error: {}", message));
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        Err(anyhow::anyhow!(
            "Connection closed before receiving debug response"
        ))
    })
    .await;

    match debug_timeout {
        Ok(Ok(response)) => {
            handle_debug_response(response)?;
        }
        Ok(Err(e)) => {
            return Err(e);
        }
        Err(_) => {
            error!("Timeout waiting for debug response after 10 seconds");
            return Err(anyhow::anyhow!(
                "Debug request timeout - the terminal may be busy or unresponsive"
            ));
        }
    }

    // Close connection
    write.send(Message::Close(None)).await?;

    Ok(())
}

/// Try to get terminal width if stdout is a TTY
fn get_terminal_width() -> Option<u16> {
    // Only check if stdout is a terminal
    if !std::io::stdout().is_terminal() {
        return None;
    }

    // Try to get terminal size using terminal_size crate or similar
    // For now, we'll use a simple approach with stty command
    use std::process::Command;

    let output = Command::new("stty").arg("size").output().ok()?;

    if !output.status.success() {
        return None;
    }

    let size = String::from_utf8(output.stdout).ok()?;
    let parts: Vec<&str> = size.trim().split_whitespace().collect();

    if parts.len() == 2 {
        parts[1].parse::<u16>().ok()
    } else {
        None
    }
}

fn handle_debug_response(response: DebugResponse) -> Result<()> {
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
            // Check terminal width and warn if too narrow
            let term_width = get_terminal_width();
            if let Some(tw) = term_width {
                if tw < width {
                    eprintln!();
                    eprintln!(
                        "⚠️  Warning: Your terminal width ({}) is smaller than the grid width ({}).",
                        tw, width
                    );
                    eprintln!(
                        "   Lines may wrap unexpectedly. Consider resizing your terminal or using a wider display."
                    );
                    eprintln!();
                }
            }

            println!("╔══════════════════════════════════════════════════════════════╗");
            println!("║                      TERMINAL GRID VIEW                      ║");
            println!("╠══════════════════════════════════════════════════════════════╣");
            println!("║ Dimensions: {}x{}", width, height);
            println!(
                "║ Cursor: ({}, {}) {}",
                cursor_row,
                cursor_col,
                if cursor_visible { "visible" } else { "hidden" }
            );
            println!("║ Lines: {} to {}", start_line, end_line);
            println!("║ Timestamp: {}", timestamp);
            println!("╠══════════════════════════════════════════════════════════════╣");

            // Print the grid content - always use ANSI colors if available
            if let Some(ansi_rows) = ansi_rows {
                // Use ANSI-colored version
                for row in ansi_rows {
                    println!("║{}║", row);
                }
            } else {
                // Fallback to plain text if ANSI not available
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
            println!(
                "║ Current Dimensions: {}x{}",
                current_dimensions.0, current_dimensions.1
            );
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

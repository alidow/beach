#![recursion_limit = "1024"]

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use beach_human::cache::terminal::{TerminalGrid, unpack_cell};
use beach_human::cache::{GridCache, Seq};
use beach_human::client::terminal::{ClientError, TerminalClient};
use beach_human::model::terminal::diff::CacheUpdate;
use beach_human::server::terminal::{
    AlacrittyEmulator, Command as PtyCommand, LocalEcho, PtyProcess, PtyWriter, SpawnConfig,
    TerminalEmulator, TerminalRuntime,
};
use beach_human::session::{
    HostSession, JoinedSession, SessionConfig, SessionError, SessionHandle, SessionManager,
    TransportOffer,
};
use beach_human::sync::terminal::{TerminalDeltaStream, TerminalSync};
use beach_human::sync::{
    DeltaBatch, LaneBudget, PriorityLane, ServerHello, ServerSynchronizer, SnapshotChunk,
    SubscriptionId, SyncConfig,
};
use beach_human::telemetry::logging::{self as logctl, LogConfig, LogLevel};
use beach_human::telemetry::{self, PerfGuard};
use beach_human::transport::webrtc::WebRtcRole;
use beach_human::transport::{self, Transport, TransportError, TransportKind};
use clap::{Args, Parser, Subcommand};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;
use tokio::time::{interval, sleep};
use tracing::{Level, debug, error, info, trace, warn};
use url::Url;
use uuid::Uuid;

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("❌ {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), CliError> {
    let cli = Cli::parse();
    let log_config = cli.logging.to_config();
    logctl::init(&log_config).map_err(|err| CliError::Logging(err.to_string()))?;
    debug!(log_level = ?log_config.level, log_file = ?log_config.file, "logging configured");
    let session_base = cli.session_server;

    match cli.command {
        Some(Command::Join(args)) => handle_join(&session_base, args).await,
        Some(Command::Host(args)) => handle_host(&session_base, args).await,
        None => handle_host(&session_base, HostArgs::default()).await,
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "beach",
    about = "🏖️  Share a terminal session with WebRTC/WebSocket transports",
    author,
    version
)]
struct Cli {
    #[arg(
        long,
        global = true,
        env = "BEACH_SESSION_SERVER",
        default_value = "http://127.0.0.1:8080",
        help = "Base URL for the beach-road session broker"
    )]
    session_server: String,

    #[command(flatten)]
    logging: LoggingArgs,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Args, Debug, Clone)]
struct LoggingArgs {
    #[arg(
        long = "log-level",
        value_enum,
        env = "BEACH_LOG_LEVEL",
        default_value_t = LogLevel::Warn,
        help = "Minimum log level (error, warn, info, debug, trace)"
    )]
    level: LogLevel,

    #[arg(
        long = "log-file",
        value_name = "PATH",
        env = "BEACH_LOG_FILE",
        help = "Write structured logs to the specified file"
    )]
    file: Option<PathBuf>,
}

impl LoggingArgs {
    fn to_config(&self) -> LogConfig {
        LogConfig {
            level: self.level,
            file: self.file.clone(),
        }
    }
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Explicitly host a session (default when no subcommand given)
    Host(HostArgs),
    /// Join an existing session using a session id or share URL
    Join(JoinArgs),
}

#[derive(Args, Debug, Default)]
struct HostArgs {
    #[arg(
        long,
        value_name = "PROGRAM",
        help = "Override the shell launched for hosting (defaults to $SHELL)"
    )]
    shell: Option<String>,

    #[arg(
        trailing_var_arg = true,
        value_name = "COMMAND",
        help = "Command to run instead of the shell"
    )]
    command: Vec<String>,

    #[arg(
        long = "local-preview",
        action = clap::ArgAction::SetTrue,
        help = "Open a local preview client in this terminal"
    )]
    local_preview: bool,
}

#[derive(Args, Debug)]
struct JoinArgs {
    #[arg(value_name = "SESSION", help = "Session id or share URL")]
    target: String,

    #[arg(
        long,
        short = 'p',
        value_name = "CODE",
        help = "Six digit passcode (prompted interactively if omitted)"
    )]
    passcode: Option<String>,
}

#[derive(Debug, Error)]
enum CliError {
    #[error("{0}")]
    Session(#[from] SessionError),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("unable to determine session id from '{target}'")]
    InvalidSessionTarget { target: String },
    #[error("no executable command available; set $SHELL or pass '-- command'")]
    MissingCommand,
    #[error("session requires a six digit passcode")]
    MissingPasscode,
    #[error("transport negotiation failed: {0}")]
    TransportNegotiation(String),
    #[error("session did not provide a supported transport offer")]
    NoUsableTransport,
    #[error("terminal runtime error: {0}")]
    Runtime(String),
    #[error("logging initialization failed: {0}")]
    Logging(String),
}

async fn handle_host(base_url: &str, args: HostArgs) -> Result<(), CliError> {
    let manager = SessionManager::new(SessionConfig::new(base_url)?)?;
    let normalized_base = manager.config().base_url().to_string();
    let local_preview_enabled = args.local_preview;
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    let raw_guard = RawModeGuard::new(interactive);

    let hosted = manager.host().await?;
    let session_id = hosted.session_id().to_string();
    info!(session_id = %session_id, "session registered");
    print_host_banner(&hosted, &normalized_base, TransportKind::WebRtc);
    info!(session_id = %session_id, "waiting for WebRTC transport");
    let transport = negotiate_transport(hosted.handle(), Some(hosted.join_code())).await?;
    let selected_kind = transport.kind();
    info!(session_id = %session_id, transport = ?selected_kind, "transport negotiated");
    HeartbeatPublisher::new(transport.clone()).spawn(Duration::from_secs(5), None);

    let command = resolve_launch_command(&args)?;
    let command_display = display_cmd(&command);
    let (spawn_config, grid) = build_spawn_config(&command)?;
    let sync_config = SyncConfig::default();
    let timeline = Arc::new(TimelineDeltaStream::new());
    let delta_stream: Arc<dyn TerminalDeltaStream> = timeline.clone();
    let terminal_sync = Arc::new(TerminalSync::new(
        grid.clone(),
        delta_stream,
        sync_config.clone(),
    ));

    let emulator = Box::new(AlacrittyEmulator::new(&grid));
    let local_echo = Arc::new(LocalEcho::new());
    let (runtime, updates) = TerminalRuntime::spawn(
        spawn_config,
        emulator,
        grid.clone(),
        true,
        Some(local_echo.clone()),
    )
    .map_err(|err| CliError::Runtime(err.to_string()))?;
    let writer = runtime.writer();
    let process_handle = runtime.process_handle();
    let emulator_handle = runtime.emulator_handle();

    info!(session_id = %session_id, "host ready");

    let mut input_handles = Vec::new();
    input_handles.push(spawn_input_listener(
        transport.clone(),
        writer.clone(),
        process_handle.clone(),
        emulator_handle.clone(),
    ));

    let mut forward_transports: Vec<Arc<dyn Transport>> = vec![transport.clone()];

    let mut local_preview_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut local_server_transport: Option<Arc<dyn Transport>> = None;

    if local_preview_enabled {
        let pair = transport::TransportPair::new(TransportKind::Ipc);
        let local_client_transport: Arc<dyn Transport> = Arc::from(pair.client);
        let local_server: Arc<dyn Transport> = Arc::from(pair.server);

        input_handles.push(spawn_input_listener(
            local_server.clone(),
            writer.clone(),
            process_handle.clone(),
            emulator_handle.clone(),
        ));

        local_preview_task = Some(tokio::task::spawn_blocking(move || {
            let client = TerminalClient::new(local_client_transport).with_predictive_input(true);
            match client.run() {
                Ok(()) | Err(ClientError::Shutdown) => {}
                Err(err) => eprintln!("⚠️  preview client error: {err}"),
            }
        }));

        forward_transports.push(local_server.clone());
        local_server_transport = Some(local_server);
        debug!(session_id = %session_id, "local preview transport attached");
    }

    if interactive {
        input_handles.push(spawn_local_stdin_forwarder(
            writer.clone(),
            local_echo.clone(),
        ));
    }

    let updates_task = spawn_update_forwarder(
        forward_transports,
        updates,
        timeline.clone(),
        terminal_sync.clone(),
        sync_config.clone(),
    );

    runtime
        .wait()
        .await
        .map_err(|err| CliError::Runtime(err.to_string()))?;

    // Restore cooked mode before we print shutdown banners so the host shell
    // redraws cleanly (mirrors the legacy apps/beach behaviour).
    drop(raw_guard);

    let _ = send_json(&transport, json!({"type": "shutdown"}));
    if let Some(server) = &local_server_transport {
        let _ = send_json(server, json!({"type": "shutdown"}));
    }

    if let Err(err) = updates_task.await {
        eprintln!("⚠️  update forwarder ended unexpectedly: {err}");
    }

    if let Some(handle) = local_preview_task {
        let _ = handle.await;
    }

    for handle in input_handles {
        handle.join().ok();
    }

    println!("\n✅ command '{}' completed", command_display);
    info!(session_id = %session_id, "host command completed");
    Ok(())
}

async fn handle_join(base_url: &str, args: JoinArgs) -> Result<(), CliError> {
    let (session_id, inferred_base) = interpret_session_target(&args.target)?;
    let base = inferred_base.unwrap_or_else(|| base_url.to_string());

    let manager = SessionManager::new(SessionConfig::new(&base)?)?;
    let passcode = match args.passcode {
        Some(code) => code,
        None => prompt_passcode()?,
    };

    let trimmed_pass = passcode.trim().to_string();
    let joined = manager.join(&session_id, trimmed_pass.as_str()).await?;
    let transport = negotiate_transport(joined.handle(), Some(trimmed_pass.as_str())).await?;
    let selected_kind = transport.kind();
    info!(session_id = %joined.session_id(), transport = ?selected_kind, "joined session");
    print_join_banner(&joined, selected_kind);

    let client_transport = transport.clone();
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    tokio::task::spawn_blocking(move || {
        let _raw_guard = RawModeGuard::new(interactive);
        let client = TerminalClient::new(client_transport);
        match client.run() {
            Ok(()) | Err(ClientError::Shutdown) => {}
            Err(err) => eprintln!("⚠️  client error: {err}"),
        }
    })
    .await
    .map_err(|err| CliError::Runtime(err.to_string()))?;

    Ok(())
}

async fn negotiate_transport(
    handle: &SessionHandle,
    passphrase: Option<&str>,
) -> Result<Arc<dyn Transport>, CliError> {
    let mut errors = Vec::new();

    // Prefer WebRTC data channels for sync; fall back to WebSocket only if absolutely necessary.
    for offer in handle.offers() {
        if let TransportOffer::WebRtc { offer } = offer {
            let Some(signaling_url) = offer.get("signaling_url").and_then(Value::as_str) else {
                errors.push("webrtc offer missing signaling_url".to_string());
                continue;
            };
            let role = match offer.get("role").and_then(Value::as_str) {
                Some("offerer") => WebRtcRole::Offerer,
                Some("answerer") | None => WebRtcRole::Answerer,
                Some(other) => {
                    errors.push(format!("unsupported webrtc role {}", other));
                    continue;
                }
            };
            let poll_ms = offer
                .get("poll_interval_ms")
                .and_then(Value::as_u64)
                .unwrap_or(250);

            debug!(transport = "webrtc", signaling_url = %signaling_url, ?role, "attempting webrtc transport");
            match transport::webrtc::connect_via_signaling(
                signaling_url,
                role,
                Duration::from_millis(poll_ms),
                passphrase,
            )
            .await
            {
                Ok(transport) => {
                    info!(transport = "webrtc", signaling_url = %signaling_url, ?role, "transport established");
                    return Ok(transport);
                }
                Err(err) => {
                    warn!(transport = "webrtc", signaling_url = %signaling_url, ?role, error = %err, "webrtc negotiation failed");
                    errors.push(format!("webrtc {}: {}", signaling_url, err));
                }
            }
        }
    }

    for offer in handle.offers() {
        if let TransportOffer::WebSocket { url } = offer {
            debug!(transport = "websocket", url = %url, "attempting websocket transport");
            match transport::websocket::connect(url).await {
                Ok(transport) => {
                    info!(transport = "websocket", url = %url, "transport established");
                    return Ok(Arc::from(transport));
                }
                Err(err) => {
                    warn!(transport = "websocket", url = %url, error = %err, "websocket negotiation failed");
                    errors.push(format!("websocket {}: {}", url, err));
                }
            }
        }
    }

    if errors.is_empty() {
        Err(CliError::NoUsableTransport)
    } else {
        Err(CliError::TransportNegotiation(errors.join("; ")))
    }
}

fn resolve_launch_command(args: &HostArgs) -> Result<Vec<String>, CliError> {
    if !args.command.is_empty() {
        return Ok(args.command.clone());
    }
    if let Some(shell) = &args.shell {
        return Ok(vec![shell.clone()]);
    }
    default_shell_command().ok_or(CliError::MissingCommand)
}

fn default_shell_command() -> Option<Vec<String>> {
    if let Ok(shell) = std::env::var("SHELL") {
        if !shell.trim().is_empty() {
            return Some(vec![shell]);
        }
    }
    if cfg!(windows) {
        if let Ok(comspec) = std::env::var("COMSPEC") {
            if !comspec.trim().is_empty() {
                return Some(vec![comspec]);
            }
        }
        return Some(vec!["cmd.exe".into()]);
    }
    Some(vec!["/bin/sh".into()])
}

fn interpret_session_target(target: &str) -> Result<(String, Option<String>), CliError> {
    if let Ok(id) = Uuid::parse_str(target) {
        return Ok((id.to_string(), None));
    }

    let url = Url::parse(target).map_err(|_| CliError::InvalidSessionTarget {
        target: target.to_string(),
    })?;

    let session_id = session_id_from_url(&url).ok_or(CliError::InvalidSessionTarget {
        target: target.to_string(),
    })?;

    let base = base_from_url(&url);

    Ok((session_id, base))
}

fn session_id_from_url(url: &Url) -> Option<String> {
    let mut segments: Vec<_> = url
        .path_segments()
        .map(|segments| segments.filter(|s| !s.is_empty()).collect())
        .unwrap_or_else(Vec::new);
    if segments.is_empty() {
        return None;
    }

    if segments.last().map(|s| *s == "join").unwrap_or(false) {
        segments.pop();
    }
    let id = segments.pop()?;
    let candidate = id.to_string();
    Uuid::parse_str(&candidate).ok()?;
    Some(candidate)
}

fn base_from_url(url: &Url) -> Option<String> {
    let mut segments: Vec<String> = url
        .path_segments()
        .map(|s| {
            s.filter(|segment| !segment.is_empty())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();

    if segments.is_empty() {
        let mut base = url.clone();
        base.set_query(None);
        base.set_fragment(None);
        base.set_path("/");
        return Some(base.to_string());
    }

    if segments.last().map(|s| s == "join").unwrap_or(false) {
        segments.pop();
    }
    if !segments.is_empty() {
        segments.pop();
    }
    if segments.last().map(|s| s == "sessions").unwrap_or(false) {
        segments.pop();
    }

    let mut base = url.clone();
    base.set_query(None);
    base.set_fragment(None);
    if segments.is_empty() {
        base.set_path("/");
    } else {
        let mut path = String::new();
        for segment in &segments {
            path.push('/');
            path.push_str(segment);
        }
        path.push('/');
        base.set_path(&path);
    }
    Some(base.to_string())
}

fn prompt_passcode() -> Result<String, CliError> {
    print!("🔐 Enter passcode: ");
    io::stdout().flush()?;
    let mut buf = String::new();
    io::stdin().read_line(&mut buf)?;
    let trimmed = buf.trim();
    if trimmed.len() == 6 && trimmed.chars().all(|c| c.is_ascii_digit()) {
        Ok(trimmed.to_string())
    } else {
        Err(CliError::MissingPasscode)
    }
}

fn print_host_banner(session: &HostSession, base: &str, selected: TransportKind) {
    let handle = session.handle();
    println!("\n🏖️  beach session ready!\n");
    println!("  session id : {}", handle.session_id);
    println!("  share url  : {}", handle.session_url);
    println!("  passcode   : {}", session.join_code());
    println!(
        "\n  share command:\n    beach --session-server {} join {} --passcode {}\n",
        base,
        handle.session_id,
        session.join_code()
    );
    println!("  transports : {}", summarize_offers(handle.offers()));
    println!("  active     : {}\n", kind_label(selected));
    println!("🌊 Launching host process... type 'exit' to end the session.\n");
}

fn print_join_banner(session: &JoinedSession, selected: TransportKind) {
    let handle = session.handle();
    println!("\n🌊 Joined session {}!", handle.session_id);
    println!(
        "  transports negotiated: {}",
        summarize_offers(handle.offers())
    );
    if let Some(offer) = handle.preferred_offer() {
        println!("  preferred transport : {}", offer_label(offer));
    }
    println!("  active transport     : {}", kind_label(selected));
    println!("\nListening for session events...\n");
}

fn summarize_offers(offers: &[TransportOffer]) -> String {
    let mut labels = Vec::new();
    for offer in offers {
        let label = offer_label(offer);
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    labels.join(", ")
}

fn offer_label(offer: &TransportOffer) -> &'static str {
    match offer {
        TransportOffer::WebRtc { .. } => "WebRTC",
        TransportOffer::WebSocket { .. } => "WebSocket",
        TransportOffer::Ipc => "IPC",
    }
}

fn kind_label(kind: TransportKind) -> &'static str {
    match kind {
        TransportKind::WebRtc => "WebRTC",
        TransportKind::WebSocket => "WebSocket",
        TransportKind::Ipc => "IPC",
    }
}

#[derive(Clone)]
struct HeartbeatPublisher {
    transport: Arc<dyn Transport>,
}

impl HeartbeatPublisher {
    fn new(transport: Arc<dyn Transport>) -> Self {
        Self { transport }
    }

    fn spawn(self, interval: Duration, limit: Option<usize>) {
        tokio::spawn(async move {
            let mut count: usize = 0;
            loop {
                if let Some(max) = limit {
                    if count >= max {
                        break;
                    }
                }

                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis();
                let payload = json!({
                    "type": "heartbeat",
                    "seq": count as u64,
                    "timestamp_ms": now,
                })
                .to_string();

                if let Err(err) = self.transport.send_text(&payload) {
                    eprintln!("⚠️  heartbeat send failed: {err}");
                    break;
                }

                count += 1;
                sleep(interval).await;
            }
        });
    }
}

fn build_spawn_config(command: &[String]) -> Result<(SpawnConfig, Arc<TerminalGrid>), CliError> {
    let mut iter = command.iter();
    let program = iter.next().cloned().ok_or(CliError::MissingCommand)?;
    let args: Vec<String> = iter.cloned().collect();

    let pty_command = PtyCommand::new(program)
        .args(args)
        .env("TERM", "xterm-256color");

    let (cols, rows) = detect_terminal_size();
    let grid = Arc::new(TerminalGrid::new(rows as usize, cols as usize));
    let config = SpawnConfig::new(pty_command, cols, rows);
    Ok((config, grid))
}

fn detect_terminal_size() -> (u16, u16) {
    let cols = std::env::var("COLUMNS")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(80);
    let rows = std::env::var("LINES")
        .or_else(|_| std::env::var("ROWS"))
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(24);
    (cols.max(20), rows.max(10))
}

struct RawModeGuard(bool);

impl RawModeGuard {
    fn new(enable: bool) -> Self {
        if enable {
            match enable_raw_mode() {
                Ok(()) => Self(true),
                Err(err) => {
                    eprintln!("⚠️  failed to enable raw mode: {err}");
                    Self(false)
                }
            }
        } else {
            Self(false)
        }
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.0 {
            let _ = disable_raw_mode();
        }
    }
}

fn spawn_input_listener(
    transport: Arc<dyn Transport>,
    writer: PtyWriter,
    process: Arc<PtyProcess>,
    emulator: Arc<Mutex<Box<dyn TerminalEmulator + Send>>>,
) -> thread::JoinHandle<()> {
    let transport_id = transport.id().0;
    let transport_kind = transport.kind();
    thread::spawn(move || {
        let mut last_seq: Seq = 0;
        debug!(
            target = "sync::incoming",
            transport_id,
            transport = ?transport_kind,
            "input listener started"
        );
        loop {
            match transport.recv(Duration::from_millis(250)) {
                Ok(message) => {
                    if let Some(text) = message.payload.as_text() {
                        match serde_json::from_str::<ClientFrame>(text) {
                            Ok(ClientFrame::Input { seq, data }) => {
                                if seq <= last_seq {
                                    trace!(
                                        target = "sync::incoming",
                                        transport_id,
                                        transport = ?transport_kind,
                                        seq,
                                        "dropping duplicate input sequence"
                                    );
                                    continue;
                                }
                                match BASE64.decode(data.as_bytes()) {
                                    Ok(bytes) => {
                                        if let Err(err) = writer.write(&bytes) {
                                            error!(
                                                target = "sync::incoming",
                                                transport_id,
                                                transport = ?transport_kind,
                                                seq,
                                                error = %err,
                                                "pty write failed"
                                            );
                                            break;
                                        }
                                        if tracing::enabled!(Level::TRACE) {
                                            trace!(
                                                target = "sync::incoming",
                                                transport_id,
                                                transport = ?transport_kind,
                                                seq,
                                                bytes = bytes.len(),
                                                dump = %logctl::hexdump(&bytes),
                                                "client input bytes"
                                            );
                                        }
                                        last_seq = seq;
                                        let _ = send_json(
                                            &transport,
                                            json!({
                                                "type": "input_ack",
                                                "seq": seq,
                                            }),
                                        );
                                        debug!(
                                            target = "sync::incoming",
                                            transport_id,
                                            transport = ?transport_kind,
                                            seq,
                                            "input applied and acked"
                                        );
                                    }
                                    Err(err) => {
                                        warn!(
                                            target = "sync::incoming",
                                            transport_id,
                                            transport = ?transport_kind,
                                            seq,
                                            error = %err,
                                            "invalid input payload"
                                        );
                                    }
                                }
                            }
                            Ok(ClientFrame::Resize { cols, rows }) => {
                                if let Err(err) = process.resize(cols, rows) {
                                    warn!(
                                        target = "sync::incoming",
                                        transport_id,
                                        transport = ?transport_kind,
                                        error = %err,
                                        cols,
                                        rows,
                                        "pty resize failed"
                                    );
                                }
                                if let Ok(mut guard) = emulator.lock() {
                                    guard.resize(rows as usize, cols as usize);
                                }
                                let _ = send_json(
                                    &transport,
                                    encode_grid_descriptor(rows as usize, cols as usize),
                                );
                                debug!(
                                    target = "sync::incoming",
                                    transport_id,
                                    transport = ?transport_kind,
                                    cols,
                                    rows,
                                    "processed resize request"
                                );
                            }
                            Ok(ClientFrame::Unknown) | Err(_) => {}
                        }
                    }
                }
                Err(TransportError::Timeout) => continue,
                Err(TransportError::ChannelClosed) => break,
                Err(err) => {
                    warn!(
                        target = "sync::incoming",
                        transport_id,
                        transport = ?transport_kind,
                        error = %err,
                        "input listener error"
                    );
                    break;
                }
            }
        }
        debug!(
            target = "sync::incoming",
            transport_id,
            transport = ?transport_kind,
            "input listener stopped"
        );
    })
}

fn spawn_local_stdin_forwarder(
    writer: PtyWriter,
    local_echo: Arc<LocalEcho>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut stdin = io::stdin();
        let mut buffer = [0u8; 1024];
        loop {
            match stdin.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    let bytes = &buffer[..n];
                    if writer.write(bytes).is_err() {
                        break;
                    }
                    local_echo.record_input(bytes);
                    if tracing::enabled!(Level::TRACE) {
                        trace!(
                            target = "host::stdin",
                            bytes = n,
                            dump = %logctl::hexdump(bytes),
                            "stdin forwarded to pty"
                        );
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => {
                    warn!(target = "host::stdin", error = %err, "local input error");
                    break;
                }
            }
        }
        local_echo.clear();
        trace!(target = "host::stdin", "stdin forwarder exited");
    })
}

struct TimelineDeltaStream {
    history: Mutex<VecDeque<CacheUpdate>>,
    latest: AtomicU64,
    capacity: usize,
}

impl TimelineDeltaStream {
    fn new() -> Self {
        Self {
            history: Mutex::new(VecDeque::with_capacity(1024)),
            latest: AtomicU64::new(0),
            capacity: 8192,
        }
    }

    fn record(&self, update: &CacheUpdate) {
        self.latest.store(update.seq(), Ordering::Relaxed);
        let mut history = self.history.lock().unwrap();
        history.push_back(update.clone());
        while history.len() > self.capacity {
            history.pop_front();
        }
    }
}

impl TerminalDeltaStream for TimelineDeltaStream {
    fn collect_since(&self, since: Seq, budget: usize) -> Vec<CacheUpdate> {
        let history = self.history.lock().unwrap();
        history
            .iter()
            .filter(|update| update.seq() > since)
            .take(budget)
            .cloned()
            .collect()
    }

    fn latest_seq(&self) -> Seq {
        self.latest.load(Ordering::Relaxed)
    }
}

fn spawn_update_forwarder(
    transports: Vec<Arc<dyn Transport>>,
    mut updates: UnboundedReceiver<CacheUpdate>,
    timeline: Arc<TimelineDeltaStream>,
    terminal_sync: Arc<TerminalSync>,
    sync_config: SyncConfig,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        if transports.is_empty() {
            return;
        }

        struct Sink {
            transport: Arc<dyn Transport>,
            synchronizer: ServerSynchronizer<TerminalSync, CacheUpdate>,
            last_seq: Seq,
            active: bool,
            handshake_complete: bool,
            last_handshake: Instant,
            handshake_attempts: u32,
        }

        const HANDSHAKE_REFRESH: Duration = Duration::from_millis(200);

        let subscription = SubscriptionId(1);
        let mut sinks: Vec<Sink> = transports
            .into_iter()
            .map(|transport| Sink {
                synchronizer: ServerSynchronizer::new(terminal_sync.clone(), sync_config.clone()),
                transport,
                last_seq: 0,
                active: true,
                handshake_complete: false,
                last_handshake: Instant::now(),
                handshake_attempts: 0,
            })
            .collect();

        for sink in sinks.iter_mut() {
            if let Some((sync, seq)) = initialize_transport_snapshot(
                &sink.transport,
                subscription,
                &terminal_sync,
                &sync_config,
            ) {
                sink.synchronizer = sync;
                sink.last_seq = seq;
                sink.handshake_complete = true;
            }
            sink.last_handshake = Instant::now();
        }

        fn attempt_handshake(
            sink: &mut Sink,
            subscription: SubscriptionId,
            terminal_sync: &Arc<TerminalSync>,
            sync_config: &SyncConfig,
        ) {
            sink.handshake_attempts = sink.handshake_attempts.saturating_add(1);
            debug!(
                target = "sync::handshake",
                transport_id = sink.transport.id().0,
                transport = ?sink.transport.kind(),
                attempt = sink.handshake_attempts,
                "starting handshake replay"
            );
            sink.last_handshake = Instant::now();
            if let Some((sync, seq)) = initialize_transport_snapshot(
                &sink.transport,
                subscription,
                terminal_sync,
                sync_config,
            ) {
                sink.synchronizer = sync;
                sink.last_seq = seq;
                sink.handshake_complete = true;
                debug!(
                    target = "sync::handshake",
                    transport_id = sink.transport.id().0,
                    transport = ?sink.transport.kind(),
                    watermark = seq,
                    "handshake complete"
                );
            } else {
                sink.handshake_complete = false;
                debug!(
                    target = "sync::handshake",
                    transport_id = sink.transport.id().0,
                    transport = ?sink.transport.kind(),
                    "handshake attempt did not complete"
                );
            }
        }

        let mut handshake_timer = interval(Duration::from_millis(200));

        loop {
            tokio::select! {
                _ = handshake_timer.tick() => {
                    for sink in sinks.iter_mut().filter(|s| s.active) {
                        let needs_refresh = !sink.handshake_complete
                            || sink.last_handshake.elapsed() >= HANDSHAKE_REFRESH;
                        if needs_refresh {
                            attempt_handshake(sink, subscription, &terminal_sync, &sync_config);
                        }
                    }
                }
                maybe_update = updates.recv() => {
                    match maybe_update {
                        Some(update) => {
                            timeline.record(&update);
                            trace!(target = "sync::timeline", seq = update.seq(), "recorded cache update");

                            let mut drained = 1usize;
                            while let Ok(extra) = updates.try_recv() {
                                trace!(target = "sync::timeline", seq = extra.seq(), "recorded coalesced update");
                                timeline.record(&extra);
                                drained = drained.saturating_add(1);
                            }
                            telemetry::record_gauge("sync_updates_batch", drained as u64);

                            for sink in sinks.iter_mut().filter(|s| s.active && s.handshake_complete) {
                                let mut batches_sent = 0usize;
                                loop {
                                    let Some(batch) = sink.synchronizer.delta_batch(subscription, sink.last_seq) else { break; };
                                    if batch.updates.is_empty() {
                                        if batch.has_more {
                                            continue;
                                        }
                                        break;
                                    }
                                    telemetry::record_gauge("sync_delta_batch_updates", batch.updates.len() as u64);
                                    let _guard = PerfGuard::new("sync_send_delta");
                                    if !send_json(&sink.transport, encode_delta_batch(&batch)) {
                                        sink.handshake_complete = false;
                                        warn!(
                                            target = "sync::handshake",
                                            transport_id = sink.transport.id().0,
                                            transport = ?sink.transport.kind(),
                                            "delta send failed, marking handshake incomplete"
                                        );
                                        break;
                                    }
                                    sink.last_seq = batch.watermark.0;
                                    sink.last_handshake = Instant::now();
                                    batches_sent = batches_sent.saturating_add(1);
                                    trace!(
                                        target = "sync::timeline",
                                        transport_id = sink.transport.id().0,
                                        transport = ?sink.transport.kind(),
                                        watermark = batch.watermark.0,
                                        updates = batch.updates.len(),
                                        has_more = batch.has_more,
                                        "delta batch delivered"
                                    );
                                    if !batch.has_more || batches_sent > 32 {
                                        break;
                                    }
                                }
                                telemetry::record_gauge("sync_delta_batches_sent", batches_sent as u64);
                            }
                        }
                        None => break,
                    }
                }
            }
        }
    })
}

fn initialize_transport_snapshot(
    transport: &Arc<dyn Transport>,
    subscription: SubscriptionId,
    terminal_sync: &Arc<TerminalSync>,
    sync_config: &SyncConfig,
) -> Option<(ServerSynchronizer<TerminalSync, CacheUpdate>, Seq)> {
    let mut synchronizer = ServerSynchronizer::new(terminal_sync.clone(), sync_config.clone());
    let hello = synchronizer.hello(subscription);
    debug!(
        target = "sync::handshake",
        transport_id = transport.id().0,
        transport = ?transport.kind(),
        "sending server hello"
    );
    if !send_json(transport, encode_server_hello(&hello)) {
        return None;
    }
    let (rows, cols) = terminal_sync.grid().dims();
    debug!(
        target = "sync::handshake",
        transport_id = transport.id().0,
        transport = ?transport.kind(),
        rows,
        cols,
        "sending grid descriptor"
    );
    if !send_json(transport, encode_grid_descriptor(rows, cols)) {
        return None;
    }
    if !transmit_initial_snapshots(transport, &mut synchronizer, subscription) {
        return None;
    }
    trace!(
        target = "sync::handshake",
        transport_id = transport.id().0,
        transport = ?transport.kind(),
        watermark = hello.max_seq.0,
        "initial snapshots transmitted"
    );
    Some((synchronizer, hello.max_seq.0))
}

fn encode_update(update: &CacheUpdate) -> Value {
    match update {
        CacheUpdate::Cell(cell) => {
            let (ch, style) = unpack_cell(cell.cell);
            json!({
                "kind": "cell",
                "row": cell.row,
                "col": cell.col,
                "seq": cell.seq,
                "char": ch.to_string(),
                "style": style.0,
            })
        }
        CacheUpdate::Rect(rect) => {
            let (ch, style) = unpack_cell(rect.cell);
            json!({
                "kind": "rect",
                "rows": [rect.rows.start, rect.rows.end],
                "cols": [rect.cols.start, rect.cols.end],
                "seq": rect.seq,
                "char": ch.to_string(),
                "style": style.0,
            })
        }
        CacheUpdate::Row(row) => {
            let mut text = String::with_capacity(row.cells.len());
            let cells: Vec<Value> = row
                .cells
                .iter()
                .map(|cell| {
                    let (ch, style) = unpack_cell(*cell);
                    text.push(ch);
                    json!({
                        "ch": ch.to_string(),
                        "style": style.0,
                    })
                })
                .collect();
            json!({
                "kind": "row",
                "row": row.row,
                "seq": row.seq,
                "text": text,
                "cells": cells,
            })
        }
        CacheUpdate::Trim(trim) => {
            json!({
                "kind": "trim",
                "start": trim.start,
                "count": trim.count,
            })
        }
    }
}

fn lane_label(lane: PriorityLane) -> &'static str {
    match lane {
        PriorityLane::Foreground => "foreground",
        PriorityLane::Recent => "recent",
        PriorityLane::History => "history",
    }
}

fn send_json(transport: &Arc<dyn Transport>, value: Value) -> bool {
    let frame_type = value
        .get("type")
        .and_then(Value::as_str)
        .map(|s| s.to_owned())
        .unwrap_or_else(|| "unknown".to_string());

    match serde_json::to_string(&value) {
        Ok(text) => {
            if tracing::enabled!(Level::DEBUG) {
                let transport_id = transport.id().0;
                let transport_kind = transport.kind();
                debug!(
                    target = "sync::outgoing",
                    transport_id,
                    transport = ?transport_kind,
                    frame = %frame_type,
                    "sending frame"
                );
                if tracing::enabled!(Level::TRACE) {
                    trace!(
                        target = "sync::outgoing",
                        transport_id,
                        transport = ?transport_kind,
                        frame = %frame_type,
                        payload = %text,
                        "frame payload"
                    );
                }
            }

            telemetry::record_bytes("sync_send_bytes", text.len());
            let _guard = PerfGuard::new("sync_send_json");
            match transport.send_text(&text) {
                Ok(_) => true,
                Err(TransportError::ChannelClosed) => false,
                Err(err) => {
                    warn!(
                        target = "sync::outgoing",
                        transport_id = transport.id().0,
                        transport = ?transport.kind(),
                        frame = %frame_type,
                        error = %err,
                        "transport send failed"
                    );
                    false
                }
            }
        }
        Err(err) => {
            error!(target = "sync::outgoing", frame = %frame_type, error = %err, "failed to encode message");
            false
        }
    }
}

fn encode_sync_config(config: &SyncConfig) -> Value {
    let budgets: Vec<Value> = config
        .snapshot_budgets
        .iter()
        .map(|LaneBudget { lane, max_updates }| {
            json!({
                "lane": lane_label(*lane),
                "max_updates": max_updates,
            })
        })
        .collect();

    json!({
        "snapshot_budgets": budgets,
        "delta_budget": config.delta_budget,
        "heartbeat_ms": config.heartbeat_interval.as_millis(),
    })
}

fn encode_grid_descriptor(rows: usize, cols: usize) -> Value {
    json!({
        "type": "grid",
        "rows": rows,
        "cols": cols,
    })
}

fn encode_server_hello(hello: &ServerHello) -> Value {
    json!({
        "type": "hello",
        "subscription": hello.subscription_id.0,
        "max_seq": hello.max_seq.0,
        "config": encode_sync_config(&hello.config),
    })
}

fn encode_snapshot_chunk(chunk: &SnapshotChunk<CacheUpdate>) -> Value {
    json!({
        "type": "snapshot",
        "subscription": chunk.subscription_id.0,
        "lane": lane_label(chunk.lane),
        "watermark": chunk.watermark.0,
        "has_more": chunk.has_more,
        "updates": chunk.updates.iter().map(encode_update).collect::<Vec<_>>(),
    })
}

fn encode_delta_updates(updates: &[CacheUpdate]) -> Vec<Value> {
    use serde_json::Value;

    let mut out = Vec::with_capacity(updates.len());
    let mut segment_row: Option<usize> = None;
    let mut segment_cells: Vec<Value> = Vec::new();

    fn flush_segment(out: &mut Vec<Value>, row: &mut Option<usize>, cells: &mut Vec<Value>) {
        if let Some(r) = row.take() {
            if !cells.is_empty() {
                let taken = std::mem::take(cells);
                out.push(json!({
                    "kind": "segment",
                    "row": r,
                    "cells": taken,
                }));
            }
        } else {
            cells.clear();
        }
    }

    for update in updates {
        match update {
            CacheUpdate::Cell(cell) => {
                let (ch, style) = unpack_cell(cell.cell);
                if segment_row != Some(cell.row) {
                    flush_segment(&mut out, &mut segment_row, &mut segment_cells);
                    segment_row = Some(cell.row);
                }
                segment_cells.push(json!({
                    "col": cell.col,
                    "seq": cell.seq,
                    "char": ch.to_string(),
                    "style": style.0,
                }));
            }
            other => {
                flush_segment(&mut out, &mut segment_row, &mut segment_cells);
                out.push(encode_update(other));
            }
        }
    }

    flush_segment(&mut out, &mut segment_row, &mut segment_cells);
    out
}

fn encode_snapshot_complete(subscription: SubscriptionId, lane: PriorityLane) -> Value {
    json!({
        "type": "snapshot_complete",
        "subscription": subscription.0,
        "lane": lane_label(lane),
    })
}

fn encode_delta_batch(batch: &DeltaBatch<CacheUpdate>) -> Value {
    json!({
        "type": "delta",
        "subscription": batch.subscription_id.0,
        "watermark": batch.watermark.0,
        "has_more": batch.has_more,
        "updates": encode_delta_updates(&batch.updates),
    })
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientFrame {
    Input {
        seq: Seq,
        data: String,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    #[serde(other)]
    Unknown,
}

fn transmit_initial_snapshots(
    transport: &Arc<dyn Transport>,
    synchronizer: &mut ServerSynchronizer<TerminalSync, CacheUpdate>,
    subscription: SubscriptionId,
) -> bool {
    let transport_id = transport.id().0;
    let transport_kind = transport.kind();
    for lane in [
        PriorityLane::Foreground,
        PriorityLane::Recent,
        PriorityLane::History,
    ] {
        while let Some(chunk) = synchronizer.snapshot_chunk(subscription, lane) {
            debug!(
                target = "sync::handshake",
                transport_id,
                transport = ?transport_kind,
                lane = ?lane,
                updates = chunk.updates.len(),
                "sending snapshot chunk"
            );
            if !send_json(transport, encode_snapshot_chunk(&chunk)) {
                return false;
            }
            if !chunk.has_more {
                trace!(
                    target = "sync::handshake",
                    transport_id,
                    transport = ?transport_kind,
                    lane = ?lane,
                    "lane snapshot complete"
                );
                if !send_json(transport, encode_snapshot_complete(subscription, lane)) {
                    return false;
                }
            }
        }
    }
    true
}

fn display_cmd(cmd: &[String]) -> String {
    let mut rendered = String::new();
    for item in cmd {
        if rendered.is_empty() {
            rendered.push_str(item);
            continue;
        }
        rendered.push(' ');
        if item.chars().any(char::is_whitespace) {
            write!(&mut rendered, "\"{}\"", item).ok();
        } else {
            rendered.push_str(item);
        }
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{TransportKind, TransportPair};
    use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
    use beach_human::cache::terminal::{Style, StyleId};
    use beach_human::model::terminal::diff::RowSnapshot;
    use beach_human::sync::terminal::NullTerminalDeltaStream;
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::{Duration as StdDuration, Instant};
    use tokio::time::{Instant as TokioInstant, sleep, timeout};

    fn emit_row_update(
        grid: &Arc<TerminalGrid>,
        style_id: StyleId,
        seq: Seq,
        row: usize,
        cols: usize,
        text: &str,
    ) -> CacheUpdate {
        let chars: Vec<char> = text.chars().collect();
        let mut packed_row = Vec::with_capacity(cols);
        for col in 0..cols {
            let ch = chars.get(col).copied().unwrap_or(' ');
            let packed = TerminalGrid::pack_char_with_style(ch, style_id);
            grid.write_packed_cell_if_newer(row, col, seq, packed)
                .expect("write cell");
            packed_row.push(packed);
        }
        CacheUpdate::Row(RowSnapshot::new(row, seq, packed_row))
    }

    async fn recv_json_frame(transport: &Arc<dyn Transport>, timeout: StdDuration) -> Value {
        let deadline = TokioInstant::now() + timeout;
        loop {
            match transport.try_recv() {
                Ok(Some(message)) => {
                    if let Some(text) = message.payload.as_text() {
                        return serde_json::from_str(text).expect("json frame");
                    }
                }
                Ok(None) => {}
                Err(TransportError::ChannelClosed) => {
                    panic!("transport channel closed")
                }
                Err(err) => panic!("transport error: {err}"),
            }
            if TokioInstant::now() >= deadline {
                panic!("timed out waiting for frame");
            }
            sleep(StdDuration::from_millis(10)).await;
        }
    }

    #[test]
    fn parse_plain_session_id() {
        let id = Uuid::new_v4().to_string();
        let (parsed, base) = interpret_session_target(&id).unwrap();
        assert_eq!(parsed, id);
        assert!(base.is_none());
    }

    #[test]
    fn parse_url_with_join_suffix() {
        let id = Uuid::new_v4();
        let url = format!("https://example.com/api/sessions/{}/join", id);
        let (parsed, base) = interpret_session_target(&url).unwrap();
        assert_eq!(parsed, id.to_string());
        assert_eq!(base.unwrap(), "https://example.com/api/");
    }

    #[test]
    fn parse_url_without_sessions_segment() {
        let id = Uuid::new_v4();
        let url = format!("https://example.com/{id}");
        let (parsed, base) = interpret_session_target(&url).unwrap();
        assert_eq!(parsed, id.to_string());
        assert_eq!(base.unwrap(), "https://example.com/");
    }

    #[test]
    fn reject_non_uuid_target() {
        let err = interpret_session_target("not-a-session").unwrap_err();
        assert!(matches!(err, CliError::InvalidSessionTarget { .. }));
    }

    #[test_timeout::tokio_timeout_test]
    async fn webrtc_mock_session_flow() {
        timeout(StdDuration::from_secs(30), async {
            let events = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));

            let pair = transport::webrtc::create_test_pair()
                .await
                .expect("create webrtc pair");
            let client: Arc<dyn Transport> = Arc::from(pair.client);
            let server: Arc<dyn Transport> = Arc::from(pair.server);

            let rows = 24usize;
            let cols = 80usize;
            let grid = Arc::new(TerminalGrid::new(rows, cols));
            let style_id = grid.ensure_style_id(Style::default());

            // Seed prompt prior to handshake.
            let initial_prompt = "(base) host% ";
            let prompt_trimmed = initial_prompt.trim_end();
            emit_row_update(&grid, style_id, 1, rows - 1, cols, initial_prompt);

            let timeline = Arc::new(TimelineDeltaStream::new());
            let delta_stream: Arc<dyn TerminalDeltaStream> = timeline.clone();
            let sync_config = SyncConfig::default();
            let terminal_sync = Arc::new(TerminalSync::new(
                grid.clone(),
                delta_stream,
                sync_config.clone(),
            ));

            let (update_tx, update_rx) = tokio::sync::mpsc::unbounded_channel();
            let forwarder = spawn_update_forwarder(
                vec![Arc::clone(&server)],
                update_rx,
                timeline.clone(),
                terminal_sync.clone(),
                sync_config.clone(),
            );

            // Consume handshake frames until all lanes report completion.
            let hello = recv_json_frame(&client, StdDuration::from_secs(5)).await;
            events.lock().unwrap().push("received_hello".into());
            assert_eq!(hello["type"], "hello");
            let grid_frame = recv_json_frame(&client, StdDuration::from_secs(5)).await;
            events.lock().unwrap().push("received_grid".into());
            assert_eq!(grid_frame["type"], "grid");

            let mut saw_prompt = false;
            let mut foreground_complete = false;
            while !foreground_complete {
                let frame = recv_json_frame(&client, StdDuration::from_secs(5)).await;
                match frame["type"].as_str().unwrap_or("") {
                    "snapshot" => {
                        if frame["lane"] == "foreground" {
                            if let Some(updates) = frame["updates"].as_array() {
                                for update in updates {
                                    if update["kind"] == "row" {
                                        if let Some(text) = update["text"].as_str() {
                                            if text.trim_end() == prompt_trimmed {
                                                saw_prompt = true;
                                                events
                                                    .lock()
                                                    .unwrap()
                                                    .push("foreground_prompt".into());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    "snapshot_complete" => {
                        if frame["lane"] == "foreground" {
                            foreground_complete = true;
                            events.lock().unwrap().push("foreground_complete".into());
                        }
                    }
                    _ => {}
                }
            }
            assert!(saw_prompt, "foreground snapshot missing prompt");

            // Emit server-side deltas.
            let mut seq: Seq = 2;
            let command_update = emit_row_update(
                &grid,
                style_id,
                seq,
                rows - 1,
                cols,
                "(base) host% echo hello",
            );
            timeline.record(&command_update);
            update_tx
                .send(command_update)
                .expect("queue command update");
            events.lock().unwrap().push("server_command_sent".into());
            seq += 1;
            let output_update = emit_row_update(&grid, style_id, seq, rows - 2, cols, "hello");
            timeline.record(&output_update);
            update_tx.send(output_update).expect("queue output update");
            events.lock().unwrap().push("server_output_sent".into());

            let deadline = TokioInstant::now() + StdDuration::from_secs(5);
            let mut saw_command = false;
            let mut saw_output = false;
            while TokioInstant::now() < deadline && !(saw_command && saw_output) {
                let frame = recv_json_frame(&client, StdDuration::from_secs(5)).await;
                match frame["type"].as_str().unwrap_or("") {
                    "delta" => {
                        if let Some(updates) = frame["updates"].as_array() {
                            for update in updates {
                                if let Some(text) = update["text"].as_str() {
                                    let trimmed = text.trim_end();
                                    if trimmed.contains("echo hello") {
                                        saw_command = true;
                                        events.lock().unwrap().push("client_saw_command".into());
                                    }
                                    if trimmed == "hello" {
                                        saw_output = true;
                                        events.lock().unwrap().push("client_saw_output".into());
                                    }
                                }
                            }
                        }
                    }
                    "heartbeat" => continue,
                    _ => {}
                }
            }
            assert!(saw_command, "delta missing command text");
            assert!(saw_output, "delta missing command output");

            // Client -> server input travels over the same transport.
            let input_payload = json!({
                "type": "input",
                "seq": 1,
                "data": BASE64.encode(b"echo world\n"),
            })
            .to_string();
            client.send_text(&input_payload).expect("client send input");
            events.lock().unwrap().push("client_sent_input".into());

            let server_clone = Arc::clone(&server);
            let inbound =
                tokio::task::spawn_blocking(move || server_clone.recv(StdDuration::from_secs(5)))
                    .await
                    .expect("recv join")
                    .expect("server recv");
            let inbound_text = inbound.payload.as_text().expect("text payload").to_string();
            assert_eq!(inbound_text, input_payload);
            events.lock().unwrap().push("server_received_input".into());

            drop(update_tx);
            forwarder.await.expect("forwarder join");
            let summary = events.lock().unwrap();
            println!("webrtc_mock_session_flow events: {}", summary.join(", "));
            assert!(summary.contains(&"foreground_prompt".to_string()));
            assert!(summary.contains(&"client_saw_command".to_string()));
            assert!(summary.contains(&"client_saw_output".to_string()));
            assert!(summary.contains(&"server_received_input".to_string()));
        })
        .await
        .expect("webrtc mock session timed out");
    }

    #[test_timeout::tokio_timeout_test]
    async fn heartbeat_publisher_emits_messages() {
        let pair = TransportPair::new(TransportKind::Ipc);
        let publisher_transport: Arc<dyn Transport> = Arc::from(pair.server);
        let client = pair.client;

        HeartbeatPublisher::new(publisher_transport).spawn(StdDuration::from_millis(10), Some(3));

        let handle = tokio::task::spawn_blocking(move || {
            let mut results = Vec::new();
            for _ in 0..3 {
                let message = client
                    .recv(StdDuration::from_secs(1))
                    .expect("heartbeat message");
                let text = message.payload.as_text().expect("text payload");
                results.push(text.to_string());
            }
            results
        });

        let payloads = handle.await.expect("join blocking task");
        for payload in payloads {
            let parsed: Value = serde_json::from_str(&payload).expect("json payload");
            assert_eq!(parsed["type"], "heartbeat");
        }
    }

    #[test_timeout::tokio_timeout_test]
    async fn snapshot_retries_until_client_receives_prompt() {
        let rows = 4;
        let cols = 16;

        let pair = TransportPair::new(TransportKind::Ipc);
        let client_transport = pair.client;
        let host_transport: Arc<dyn Transport> = Arc::from(pair.server);

        let grid = Arc::new(TerminalGrid::new(rows, cols));
        let style_id = grid.ensure_style_id(Style::default());
        let timeline = Arc::new(TimelineDeltaStream::new());
        let sync_config = SyncConfig::default();
        let delta_stream: Arc<dyn TerminalDeltaStream> = timeline.clone();
        let terminal_sync = Arc::new(TerminalSync::new(
            grid.clone(),
            delta_stream,
            sync_config.clone(),
        ));

        // Seed the grid with an existing prompt before any transport handshake.
        let prompt = "host% ";
        let _prompt_trimmed = prompt.trim_end();
        let seq: Seq = 1;
        let mut packed = Vec::new();
        for (col, ch) in prompt.chars().enumerate() {
            let cell = TerminalGrid::pack_char_with_style(ch, style_id);
            grid.write_packed_cell_if_newer(rows - 1, col, seq, cell)
                .expect("write prompt cell");
            packed.push(cell);
        }
        let update = CacheUpdate::Row(RowSnapshot::new(rows - 1, seq, packed));
        timeline.record(&update);

        let (update_tx, update_rx) = tokio::sync::mpsc::unbounded_channel();
        let forwarder = spawn_update_forwarder(
            vec![host_transport.clone()],
            update_rx,
            timeline.clone(),
            terminal_sync.clone(),
            sync_config.clone(),
        );

        // Intentionally drop the first few handshake frames (hello, grid, snapshot,
        // snapshot_complete) to mimic a client that misses the initial burst.
        tokio::task::spawn_blocking(move || {
            let mut dropped = 0;
            while dropped < 4 {
                match client_transport.recv(StdDuration::from_millis(200)) {
                    Ok(_) => dropped += 1,
                    Err(TransportError::Timeout) => continue,
                    Err(err) => panic!("transport error while dropping frames: {err:?}"),
                }
            }

            let deadline = Instant::now() + StdDuration::from_secs(3);
            let mut saw_prompt = false;
            while Instant::now() < deadline {
                match client_transport.recv(StdDuration::from_millis(200)) {
                    Ok(message) => {
                        if let Some(text) = message.payload.as_text() {
                            if let Ok(value) = serde_json::from_str::<Value>(text) {
                                if let Some(kind) = value.get("type").and_then(Value::as_str) {
                                    if kind == "snapshot" || kind == "delta" {
                                        if let Some(updates) =
                                            value.get("updates").and_then(Value::as_array)
                                        {
                                            let mut match_found = false;
                                            for entry in updates {
                                                match entry.get("kind").and_then(Value::as_str) {
                                                    Some("row") => {
                                                        if entry
                                                            .get("text")
                                                            .and_then(Value::as_str)
                                                            .map(|s| s.trim_end())
                                                            == Some("host%")
                                                        {
                                                            match_found = true;
                                                            break;
                                                        }
                                                    }
                                                    Some("segment") => {
                                                        if let Some(cells) = entry
                                                            .get("cells")
                                                            .and_then(Value::as_array)
                                                        {
                                                            let mut buffer = String::new();
                                                            for cell in cells {
                                                                if let Some(ch) = cell
                                                                    .get("char")
                                                                    .and_then(Value::as_str)
                                                                {
                                                                    buffer.push_str(ch);
                                                                }
                                                            }
                                                            if buffer.trim_end() == "host%" {
                                                                match_found = true;
                                                                break;
                                                            }
                                                        }
                                                    }
                                                    _ => {}
                                                }
                                            }
                                            if match_found {
                                                saw_prompt = true;
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(TransportError::Timeout) => continue,
                    Err(err) => panic!("transport error while waiting for retry: {err:?}"),
                }
            }

            assert!(
                saw_prompt,
                "client never received prompt snapshot after retries"
            );
        })
        .await
        .expect("snapshot retry assertion");

        drop(update_tx);
        forwarder.await.expect("forwarder join");
    }

    #[test_timeout::tokio_timeout_test]
    async fn handshake_snapshot_contains_prompt_row() {
        let rows = 24;
        let cols = 80;
        let grid = Arc::new(TerminalGrid::new(rows, cols));
        let style_id = grid.ensure_style_id(Style::default());
        let prompt = "host% ";
        let prompt_trimmed = prompt.trim_end();
        for (col, ch) in prompt.chars().enumerate() {
            let packed = TerminalGrid::pack_char_with_style(ch, style_id);
            grid.write_packed_cell_if_newer(rows - 1, col, (col as Seq) + 1, packed)
                .expect("write prompt cell");
        }

        let sync_config = SyncConfig::default();
        let delta_stream: Arc<dyn TerminalDeltaStream> = Arc::new(NullTerminalDeltaStream);
        let terminal_sync = Arc::new(TerminalSync::new(
            grid.clone(),
            delta_stream,
            sync_config.clone(),
        ));

        let pair = TransportPair::new(TransportKind::Ipc);
        let host_transport: Arc<dyn Transport> = Arc::from(pair.server);
        let client_transport: Arc<dyn Transport> = Arc::from(pair.client);

        let subscription = SubscriptionId(1);
        let mut synchronizer = ServerSynchronizer::new(terminal_sync.clone(), sync_config.clone());
        let hello = synchronizer.hello(subscription);
        assert!(send_json(&host_transport, encode_server_hello(&hello)));
        assert!(send_json(
            &host_transport,
            encode_grid_descriptor(rows, cols)
        ));
        assert!(transmit_initial_snapshots(
            &host_transport,
            &mut synchronizer,
            subscription
        ));

        let mut saw_prompt = false;
        let mut prompt_cells: BTreeMap<usize, char> = BTreeMap::new();
        let target_row = rows - 1;
        for _ in 0..20 {
            match client_transport.recv(StdDuration::from_millis(200)) {
                Ok(message) => {
                    if let Some(text) = message.payload.as_text() {
                        let frame: Value = serde_json::from_str(text).expect("json frame");
                        if frame["type"] == "snapshot" {
                            if let Some(updates) = frame["updates"].as_array() {
                                for update in updates {
                                    if update["kind"] == "row" {
                                        if let Some(text) = update["text"].as_str() {
                                            if text.trim_end() == prompt_trimmed {
                                                saw_prompt = true;
                                                break;
                                            }
                                        } else if let Some(cells) = update["cells"].as_array() {
                                            let row: String = cells
                                                .iter()
                                                .enumerate()
                                                .map(|(idx, cell)| {
                                                    let ch_opt = cell
                                                        .get("ch")
                                                        .and_then(Value::as_str)
                                                        .or_else(|| {
                                                            cell.get("char").and_then(Value::as_str)
                                                        });
                                                    let ch = ch_opt
                                                        .and_then(|s| s.chars().next())
                                                        .unwrap_or(' ');
                                                    let col = cell
                                                        .get("col")
                                                        .and_then(Value::as_u64)
                                                        .map(|v| v as usize)
                                                        .unwrap_or(idx);
                                                    prompt_cells.insert(col, ch);
                                                    ch
                                                })
                                                .collect();
                                            if row.trim_end() == prompt_trimmed {
                                                saw_prompt = true;
                                                break;
                                            }
                                        }
                                    } else if update["kind"] == "cell" {
                                        if update["row"].as_u64() == Some(target_row as u64) {
                                            if let (Some(col), Some(ch_str)) =
                                                (update["col"].as_u64(), update["char"].as_str())
                                            {
                                                if let Some(ch) = ch_str.chars().next() {
                                                    prompt_cells.insert(col as usize, ch);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Err(TransportError::Timeout) => {}
                Err(err) => panic!("transport error: {err}"),
            }
            if saw_prompt {
                break;
            }
        }

        if !saw_prompt {
            let candidate: String = prompt_cells.values().copied().collect::<String>();
            if candidate.trim_end() == prompt_trimmed {
                saw_prompt = true;
            }
        }

        assert!(saw_prompt, "snapshot did not include prompt row");
    }
}

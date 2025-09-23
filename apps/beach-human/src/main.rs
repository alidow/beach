#![recursion_limit = "1024"]

use beach_human::cache::terminal::TerminalGrid;
use beach_human::cache::{GridCache, Seq};
use beach_human::client::terminal::{ClientError, TerminalClient};
use beach_human::model::terminal::diff::CacheUpdate;
use beach_human::protocol::{
    self, ClientFrame as WireClientFrame, HostFrame, Lane as WireLane,
    LaneBudgetFrame as WireLaneBudget, SyncConfigFrame as WireSyncConfig, Update as WireUpdate,
};
use beach_human::server::terminal::{
    AlacrittyEmulator, Command as PtyCommand, LocalEcho, PtyProcess, PtyWriter, SpawnConfig,
    TerminalEmulator, TerminalRuntime,
};
use beach_human::session::{
    HostSession, JoinedSession, SessionConfig, SessionError, SessionHandle, SessionManager,
    TransportOffer,
};
use beach_human::sync::terminal::{TerminalDeltaStream, TerminalSync};
use beach_human::sync::{LaneBudget, PriorityLane, ServerSynchronizer, SubscriptionId, SyncConfig};
use beach_human::telemetry::logging::{self as logctl, LogConfig, LogLevel};
use beach_human::telemetry::{self, PerfGuard};
use beach_human::transport::webrtc::WebRtcRole;
use beach_human::transport::{self, Payload, Transport, TransportError, TransportKind};
use clap::{Args, Parser, Subcommand};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
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

    let _ = send_host_frame(&transport, HostFrame::Shutdown);
    if let Some(server) = &local_server_transport {
        let _ = send_host_frame(server, HostFrame::Shutdown);
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
                if !send_host_frame(
                    &self.transport,
                    HostFrame::Heartbeat {
                        seq: count as u64,
                        timestamp_ms: now as u64,
                    },
                ) {
                    eprintln!("⚠️  heartbeat send failed");
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

fn send_host_frame(transport: &Arc<dyn Transport>, frame: HostFrame) -> bool {
    let encode_start = Instant::now();
    let bytes = protocol::encode_host_frame_binary(&frame);
    let elapsed = encode_start.elapsed();
    match &frame {
        HostFrame::Snapshot { .. } => telemetry::record_duration("sync_encode_snapshot", elapsed),
        HostFrame::Delta { .. } => telemetry::record_duration("sync_encode_delta", elapsed),
        _ => telemetry::record_duration("sync_encode_frame", elapsed),
    }
    transport.send_bytes(&bytes).is_ok()
}

fn map_lane(lane: PriorityLane) -> WireLane {
    match lane {
        PriorityLane::Foreground => WireLane::Foreground,
        PriorityLane::Recent => WireLane::Recent,
        PriorityLane::History => WireLane::History,
    }
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
                Ok(message) => match message.payload {
                    Payload::Binary(bytes) => match protocol::decode_client_frame_binary(&bytes) {
                        Ok(WireClientFrame::Input { seq, data }) => {
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
                            if let Err(err) = writer.write(&data) {
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
                                    bytes = data.len(),
                                    dump = %logctl::hexdump(&data),
                                    "client input bytes"
                                );
                            }
                            last_seq = seq;
                            let _ = send_host_frame(&transport, HostFrame::InputAck { seq });
                            debug!(
                                target = "sync::incoming",
                                transport_id,
                                transport = ?transport_kind,
                                seq,
                                "input applied and acked"
                            );
                        }
                        Ok(WireClientFrame::Resize { cols, rows }) => {
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
                            let _ = send_host_frame(
                                &transport,
                                HostFrame::Grid {
                                    rows: rows as u32,
                                    cols: cols as u32,
                                },
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
                        Ok(WireClientFrame::Unknown) => {}
                        Err(err) => {
                            warn!(
                                target = "sync::incoming",
                                transport_id,
                                transport = ?transport_kind,
                                error = %err,
                                "failed to decode client frame"
                            );
                        }
                    },
                    Payload::Text(text) => {
                        let trimmed = text.trim();
                        if trimmed == "__ready__" || trimmed == "__offer_ready__" {
                            trace!(
                                target = "sync::incoming",
                                transport_id,
                                transport = ?transport_kind,
                                "ignoring handshake sentinel"
                            );
                        } else {
                            debug!(
                                target = "sync::incoming",
                                transport_id,
                                transport = ?transport_kind,
                                payload = %trimmed,
                                "ignoring unexpected text payload"
                            );
                        }
                    }
                },
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

#[derive(Debug, Default)]
struct TransmitterCache {
    cols: usize,
    rows: HashMap<usize, Vec<u64>>,
    styles: HashMap<u32, (u32, u32, u8)>,
}

impl TransmitterCache {
    fn new() -> Self {
        Self::default()
    }

    fn reset(&mut self, cols: usize) {
        self.cols = cols;
        self.rows.clear();
        self.styles.clear();
    }

    fn apply_updates(&mut self, updates: &[CacheUpdate], dedupe: bool) -> Vec<WireUpdate> {
        let mut out = Vec::with_capacity(updates.len());
        for update in updates {
            match update {
                CacheUpdate::Row(row) => {
                    let cells: Vec<u64> = row.cells.iter().map(|c| (*c).into()).collect();
                    let changed = if dedupe {
                        self.rows
                            .get(&row.row)
                            .map(|existing| existing != &cells)
                            .unwrap_or(true)
                    } else {
                        true
                    };
                    self.cols = self.cols.max(cells.len());
                    self.rows.insert(row.row, cells.clone());
                    if changed {
                        out.push(WireUpdate::Row {
                            row: usize_to_u32(row.row),
                            seq: row.seq,
                            cells,
                        });
                    }
                }
                CacheUpdate::Rect(rect) => {
                    let mut changed = !dedupe;
                    let value: u64 = rect.cell.into();
                    self.cols = self.cols.max(rect.cols.end);
                    for r in rect.rows.clone() {
                        let row_vec = self.ensure_row_capacity(r, rect.cols.end);
                        for c in rect.cols.clone() {
                            if dedupe && !changed && row_vec[c] != value {
                                changed = true;
                            }
                            row_vec[c] = value;
                        }
                    }
                    if changed {
                        out.push(WireUpdate::Rect {
                            rows: [usize_to_u32(rect.rows.start), usize_to_u32(rect.rows.end)],
                            cols: [usize_to_u32(rect.cols.start), usize_to_u32(rect.cols.end)],
                            seq: rect.seq,
                            cell: value,
                        });
                    }
                }
                CacheUpdate::Cell(cell) => {
                    let value: u64 = cell.cell.into();
                    let row_vec = self.ensure_row_capacity(cell.row, cell.col + 1);
                    let previous = row_vec[cell.col];
                    row_vec[cell.col] = value;
                    if !dedupe || previous != value {
                        out.push(WireUpdate::Cell {
                            row: usize_to_u32(cell.row),
                            col: usize_to_u32(cell.col),
                            seq: cell.seq,
                            cell: value,
                        });
                    }
                }
                CacheUpdate::Trim(trim) => {
                    self.trim_rows(trim.start, trim.count);
                    out.push(WireUpdate::Trim {
                        start: usize_to_u32(trim.start),
                        count: usize_to_u32(trim.count),
                        seq: trim.seq(),
                    });
                }
                CacheUpdate::Style(style) => {
                    let current = (style.style.fg, style.style.bg, style.style.attrs);
                    let prev = self.styles.insert(style.id.0, current);
                    if !dedupe || prev.map_or(true, |value| value != current) {
                        out.push(WireUpdate::Style {
                            id: style.id.0,
                            seq: style.seq,
                            fg: style.style.fg,
                            bg: style.style.bg,
                            attrs: style.style.attrs,
                        });
                    }
                }
            }
        }
        out
    }

    fn ensure_row_capacity(&mut self, row: usize, min_cols: usize) -> &mut Vec<u64> {
        let columns = self.cols.max(min_cols);
        let entry = self
            .rows
            .entry(row)
            .or_insert_with(|| vec![0; columns.max(1)]);
        if entry.len() < columns {
            entry.resize(columns, 0);
        }
        if entry.len() < min_cols {
            entry.resize(min_cols, 0);
        }
        entry
    }

    fn trim_rows(&mut self, start: usize, count: usize) {
        if count == 0 {
            return;
        }
        let end = start.saturating_add(count);
        self.rows.retain(|row, _| *row >= end);
    }
}

fn usize_to_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
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
            cache: TransmitterCache,
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
                cache: TransmitterCache::new(),
            })
            .collect();

        for sink in sinks.iter_mut() {
            if let Some((sync, seq)) = initialize_transport_snapshot(
                &sink.transport,
                subscription,
                &terminal_sync,
                &sync_config,
                &mut sink.cache,
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
                &mut sink.cache,
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
                                    let converted_updates = sink.cache.apply_updates(&batch.updates, true);
                                    let _guard = PerfGuard::new("sync_send_delta");
                                    let sent = send_host_frame(
                                        &sink.transport,
                                        HostFrame::Delta {
                                            subscription: batch.subscription_id.0,
                                            watermark: batch.watermark.0,
                                            has_more: batch.has_more,
                                            updates: converted_updates,
                                        },
                                    );
                                    if !sent {
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
    cache: &mut TransmitterCache,
) -> Option<(ServerSynchronizer<TerminalSync, CacheUpdate>, Seq)> {
    let mut synchronizer = ServerSynchronizer::new(terminal_sync.clone(), sync_config.clone());
    let hello = synchronizer.hello(subscription);
    debug!(
        target = "sync::handshake",
        transport_id = transport.id().0,
        transport = ?transport.kind(),
        "sending server hello"
    );
    if !send_host_frame(
        transport,
        HostFrame::Hello {
            subscription: hello.subscription_id.0,
            max_seq: hello.max_seq.0,
            config: sync_config_to_wire(&hello.config),
        },
    ) {
        return None;
    }
    let (rows, cols) = terminal_sync.grid().dims();
    cache.reset(cols);
    debug!(
        target = "sync::handshake",
        transport_id = transport.id().0,
        transport = ?transport.kind(),
        rows,
        cols,
        "sending grid descriptor"
    );
    if !send_host_frame(
        transport,
        HostFrame::Grid {
            rows: rows as u32,
            cols: cols as u32,
        },
    ) {
        return None;
    }
    if !transmit_initial_snapshots(transport, &mut synchronizer, cache, subscription) {
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

fn sync_config_to_wire(config: &SyncConfig) -> WireSyncConfig {
    let snapshot_budgets = config
        .snapshot_budgets
        .iter()
        .map(|LaneBudget { lane, max_updates }| WireLaneBudget {
            lane: map_lane(*lane),
            max_updates: *max_updates as u32,
        })
        .collect();

    WireSyncConfig {
        snapshot_budgets,
        delta_budget: config.delta_budget as u32,
        heartbeat_ms: config.heartbeat_interval.as_millis() as u64,
    }
}

fn transmit_initial_snapshots(
    transport: &Arc<dyn Transport>,
    synchronizer: &mut ServerSynchronizer<TerminalSync, CacheUpdate>,
    cache: &mut TransmitterCache,
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
            let lane_copy = lane;
            let updates = cache.apply_updates(&chunk.updates, false);
            if !send_host_frame(
                transport,
                HostFrame::Snapshot {
                    subscription: chunk.subscription_id.0,
                    lane: map_lane(lane_copy),
                    watermark: chunk.watermark.0,
                    has_more: chunk.has_more,
                    updates,
                },
            ) {
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
                if !send_host_frame(
                    transport,
                    HostFrame::SnapshotComplete {
                        subscription: subscription.0,
                        lane: map_lane(lane),
                    },
                ) {
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
    use crate::transport::{Payload, TransportKind, TransportPair};
    use beach_human::cache::terminal::{self, PackedCell, Style, StyleId};
    use beach_human::model::terminal::diff::{
        CellWrite, HistoryTrim, RectFill, RowSnapshot, StyleDefinition,
    };
    use beach_human::protocol::{
        self, ClientFrame as WireClientFrame, HostFrame as WireHostFrame, Lane as WireLane,
        Update as WireUpdate,
    };
    use beach_human::sync::terminal::NullTerminalDeltaStream;
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

    fn send_host_frame(transport: &dyn Transport, frame: HostFrame) {
        let bytes = protocol::encode_host_frame_binary(&frame);
        transport.send_bytes(&bytes).expect("send frame");
    }

    async fn recv_host_frame_async(
        transport: &Arc<dyn Transport>,
        timeout: StdDuration,
    ) -> WireHostFrame {
        let deadline = TokioInstant::now() + timeout;
        loop {
            match transport.try_recv() {
                Ok(Some(message)) => match message.payload {
                    Payload::Binary(bytes) => {
                        return protocol::decode_host_frame_binary(&bytes).expect("host frame");
                    }
                    Payload::Text(text) => {
                        let trimmed = text.trim();
                        if trimmed == "__ready__" || trimmed == "__offer_ready__" {
                            continue;
                        }
                    }
                },
                Ok(None) => {}
                Err(TransportError::ChannelClosed) => panic!("transport channel closed"),
                Err(err) => panic!("transport error: {err}"),
            }
            if TokioInstant::now() >= deadline {
                panic!("timed out waiting for frame");
            }
            sleep(StdDuration::from_millis(10)).await;
        }
    }

    fn recv_host_frame(transport: &dyn Transport, timeout: StdDuration) -> WireHostFrame {
        let deadline = Instant::now() + timeout;
        loop {
            match transport.recv(timeout) {
                Ok(message) => match message.payload {
                    Payload::Binary(bytes) => {
                        return protocol::decode_host_frame_binary(&bytes).expect("host frame");
                    }
                    Payload::Text(text) => {
                        let trimmed = text.trim();
                        if trimmed == "__ready__" || trimmed == "__offer_ready__" {
                            continue;
                        }
                    }
                },
                Err(TransportError::Timeout) => {
                    if Instant::now() >= deadline {
                        panic!("timed out waiting for frame");
                    }
                    continue;
                }
                Err(TransportError::ChannelClosed) => panic!("transport channel closed"),
                Err(err) => panic!("transport error: {err}"),
            }
        }
    }

    fn send_client_frame(transport: &Arc<dyn Transport>, frame: WireClientFrame) {
        let bytes = protocol::encode_client_frame_binary(&frame);
        transport.send_bytes(&bytes).expect("send client frame");
    }

    fn recv_client_frame(transport: &dyn Transport, timeout: StdDuration) -> WireClientFrame {
        let deadline = Instant::now() + timeout;
        loop {
            match transport.recv(timeout) {
                Ok(message) => match message.payload {
                    Payload::Binary(bytes) => {
                        return protocol::decode_client_frame_binary(&bytes).expect("client frame");
                    }
                    Payload::Text(text) => {
                        let trimmed = text.trim();
                        if trimmed == "__ready__" || trimmed == "__offer_ready__" {
                            continue;
                        }
                    }
                },
                Err(TransportError::Timeout) => {
                    if Instant::now() >= deadline {
                        panic!("timed out waiting for client frame");
                    }
                    continue;
                }
                Err(TransportError::ChannelClosed) => panic!("transport channel closed"),
                Err(err) => panic!("transport error: {err}"),
            }
        }
    }

    struct ClientGrid {
        rows: usize,
        cols: usize,
        cells: Vec<Vec<char>>,
    }

    impl ClientGrid {
        fn new(rows: usize, cols: usize) -> Self {
            Self {
                rows,
                cols,
                cells: vec![vec![' '; cols]; rows],
            }
        }

        fn apply_update(&mut self, update: &WireUpdate) {
            match update {
                WireUpdate::Row { row, cells, .. } => {
                    let row = *row as usize;
                    if row >= self.rows {
                        return;
                    }
                    for (col, cell) in cells.iter().enumerate().take(self.cols) {
                        self.cells[row][col] = decode_cell(*cell);
                    }
                }
                WireUpdate::Cell { row, col, cell, .. } => {
                    let row = *row as usize;
                    let col = *col as usize;
                    if row < self.rows && col < self.cols {
                        self.cells[row][col] = decode_cell(*cell);
                    }
                }
                WireUpdate::Rect {
                    rows, cols, cell, ..
                } => {
                    let row0 = rows[0] as usize;
                    let row1 = rows[1] as usize;
                    let col0 = cols[0] as usize;
                    let col1 = cols[1] as usize;
                    let ch = decode_cell(*cell);
                    for row in row0..row1.min(self.rows) {
                        for col in col0..col1.min(self.cols) {
                            self.cells[row][col] = ch;
                        }
                    }
                }
                WireUpdate::RowSegment {
                    row,
                    start_col,
                    cells,
                    ..
                } => {
                    let row = *row as usize;
                    if row >= self.rows {
                        return;
                    }
                    for (idx, cell) in cells.iter().enumerate() {
                        let col = *start_col as usize + idx;
                        if col < self.cols {
                            self.cells[row][col] = decode_cell(*cell);
                        }
                    }
                }
                WireUpdate::Trim { .. } | WireUpdate::Style { .. } => {}
            }
        }

        fn contains_row(&self, needle: &str) -> bool {
            self.cells.iter().any(|row| {
                let mut needle_chars: Vec<char> = needle.chars().collect();
                if matches!(needle_chars.last(), Some(' ')) {
                    while matches!(needle_chars.last(), Some(' ')) {
                        needle_chars.pop();
                    }
                    let prefix_len = needle_chars.len();
                    let prefix_matches = row
                        .iter()
                        .take(prefix_len)
                        .copied()
                        .eq(needle_chars.into_iter());
                    let suffix_blank = row.iter().skip(prefix_len).all(|&ch| ch == ' ');
                    prefix_matches && suffix_blank
                } else {
                    let text: String = row.iter().collect();
                    text.trim_end_matches(' ') == needle
                }
            })
        }
    }

    fn decode_cell(cell: u64) -> char {
        let packed = PackedCell::from_raw(cell);
        terminal::unpack_cell(packed).0
    }

    #[test]
    fn parse_plain_session_id() {
        let id = Uuid::new_v4().to_string();
        let (parsed, base) = interpret_session_target(&id).unwrap();
        assert_eq!(parsed, id);
        assert!(base.is_none());
    }

    #[test]
    fn transmitter_cache_dedupes_rows_and_styles() {
        fn pack_cell(ch: char) -> PackedCell {
            PackedCell::from_raw(((ch as u32 as u64) << 32) | StyleId::DEFAULT.0 as u64)
        }

        let mut cache = TransmitterCache::new();
        cache.reset(4);

        let row_update =
            CacheUpdate::Row(RowSnapshot::new(0, 1, vec![pack_cell('h'), pack_cell('i')]));
        let first_emit = cache.apply_updates(&[row_update.clone()], false);
        assert_eq!(first_emit.len(), 1, "initial row should emit");

        let second_emit = cache.apply_updates(&[row_update.clone()], true);
        assert!(second_emit.is_empty(), "duplicate row should dedupe");

        let cell_update = CacheUpdate::Cell(CellWrite::new(0, 1, 2, pack_cell('o')));
        let cell_emit = cache.apply_updates(&[cell_update.clone()], true);
        assert_eq!(cell_emit.len(), 1, "cell change should emit once");
        let repeat_cell = cache.apply_updates(&[cell_update], true);
        assert!(repeat_cell.is_empty(), "repeated cell should dedupe");

        let style = StyleDefinition::new(
            StyleId(5),
            3,
            Style {
                fg: 0x00FF00,
                bg: 0x000000,
                attrs: 0b0000_0010,
            },
        );
        let style_emit = cache.apply_updates(&[CacheUpdate::Style(style.clone())], true);
        assert_eq!(style_emit.len(), 1);
        let style_repeat = cache.apply_updates(&[CacheUpdate::Style(style)], true);
        assert!(style_repeat.is_empty(), "duplicate style should dedupe");

        let trim = CacheUpdate::Trim(HistoryTrim::new(0, 1));
        let trim_emit = cache.apply_updates(&[trim.clone()], true);
        assert_eq!(trim_emit.len(), 1, "trim should always emit");

        let rect = CacheUpdate::Rect(RectFill::new(0..1, 0..2, 4, pack_cell('x')));
        let rect_emit = cache.apply_updates(&[rect.clone()], true);
        assert_eq!(rect_emit.len(), 1, "rect change should emit");
        let rect_repeat = cache.apply_updates(&[rect], true);
        assert!(rect_repeat.is_empty(), "identical rect should dedupe");
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

            let mut client_view = ClientGrid::new(rows, cols);

            // Consume handshake frames until all lanes report completion.
            match recv_host_frame_async(&client, StdDuration::from_secs(5)).await {
                WireHostFrame::Hello { .. } => {
                    events.lock().unwrap().push("received_hello".into());
                }
                other => panic!("expected hello frame, got {other:?}"),
            }

            match recv_host_frame_async(&client, StdDuration::from_secs(5)).await {
                WireHostFrame::Grid {
                    rows: grid_rows,
                    cols: grid_cols,
                } => {
                    assert_eq!(grid_rows as usize, rows);
                    assert_eq!(grid_cols as usize, cols);
                    events.lock().unwrap().push("received_grid".into());
                }
                other => panic!("expected grid frame, got {other:?}"),
            }

            let mut saw_prompt = false;
            let mut foreground_complete = false;
            while !foreground_complete {
                let frame = recv_host_frame_async(&client, StdDuration::from_secs(5)).await;
                match frame {
                    WireHostFrame::Snapshot { lane, updates, .. } => {
                        if lane == WireLane::Foreground {
                            for update in &updates {
                                client_view.apply_update(update);
                            }
                            if client_view.contains_row(prompt_trimmed) && !saw_prompt {
                                saw_prompt = true;
                                events.lock().unwrap().push("foreground_prompt".into());
                            }
                        }
                    }
                    WireHostFrame::SnapshotComplete { lane, .. } => {
                        if lane == WireLane::Foreground {
                            foreground_complete = true;
                            events.lock().unwrap().push("foreground_complete".into());
                        }
                    }
                    WireHostFrame::Delta { updates, .. } => {
                        for update in &updates {
                            client_view.apply_update(update);
                        }
                    }
                    WireHostFrame::Heartbeat { .. } => {}
                    WireHostFrame::Hello { .. }
                    | WireHostFrame::Grid { .. }
                    | WireHostFrame::InputAck { .. }
                    | WireHostFrame::Shutdown => {}
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
                let frame = recv_host_frame_async(&client, StdDuration::from_secs(5)).await;
                match frame {
                    WireHostFrame::Delta { updates, .. }
                    | WireHostFrame::Snapshot { updates, .. } => {
                        for update in &updates {
                            client_view.apply_update(update);
                        }
                        if !saw_command && client_view.contains_row("(base) host% echo hello") {
                            saw_command = true;
                            events.lock().unwrap().push("client_saw_command".into());
                        }
                        if !saw_output && client_view.contains_row("hello") {
                            saw_output = true;
                            events.lock().unwrap().push("client_saw_output".into());
                        }
                    }
                    WireHostFrame::Heartbeat { .. } => continue,
                    WireHostFrame::SnapshotComplete { .. }
                    | WireHostFrame::Hello { .. }
                    | WireHostFrame::Grid { .. }
                    | WireHostFrame::InputAck { .. }
                    | WireHostFrame::Shutdown => {}
                }
            }
            assert!(saw_command, "delta missing command text");
            assert!(saw_output, "delta missing command output");

            // Client -> server input travels over the same transport.
            send_client_frame(
                &client,
                WireClientFrame::Input {
                    seq: 1,
                    data: b"echo world
"
                    .to_vec(),
                },
            );
            events.lock().unwrap().push("client_sent_input".into());

            let server_clone = Arc::clone(&server);
            let inbound =
                tokio::task::spawn_blocking(move || server_clone.recv(StdDuration::from_secs(5)))
                    .await
                    .expect("recv join")
                    .expect("server recv");
            let client_frame = match inbound.payload {
                Payload::Binary(bytes) => {
                    protocol::decode_client_frame_binary(&bytes).expect("client frame")
                }
                Payload::Text(text) => panic!("unexpected text payload: {text}"),
            };
            match client_frame {
                WireClientFrame::Input { data, .. } => {
                    assert_eq!(
                        data.as_slice(),
                        b"echo world
"
                    );
                }
                other => panic!("unexpected client frame: {other:?}"),
            }
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
            let mut frames = Vec::new();
            for _ in 0..3 {
                let message = client
                    .recv(StdDuration::from_secs(1))
                    .expect("heartbeat message");
                match message.payload {
                    Payload::Binary(bytes) => {
                        frames.push(
                            protocol::decode_host_frame_binary(&bytes).expect("heartbeat frame"),
                        );
                    }
                    Payload::Text(text) => panic!("unexpected text payload: {text}"),
                }
            }
            frames
        });

        let frames = handle.await.expect("join blocking task");
        for frame in frames {
            match frame {
                WireHostFrame::Heartbeat { .. } => {}
                other => panic!("unexpected frame: {other:?}"),
            }
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
            let mut view = ClientGrid::new(rows as usize, cols as usize);
            while Instant::now() < deadline {
                match client_transport.recv(StdDuration::from_millis(200)) {
                    Ok(message) => match message.payload {
                        Payload::Binary(bytes) => {
                            match protocol::decode_host_frame_binary(&bytes).expect("host frame") {
                                WireHostFrame::Snapshot { updates, .. }
                                | WireHostFrame::Delta { updates, .. } => {
                                    for update in &updates {
                                        view.apply_update(update);
                                    }
                                    if view.contains_row("host%") {
                                        saw_prompt = true;
                                        break;
                                    }
                                }
                                _ => {}
                            }
                        }
                        Payload::Text(text) => {
                            let trimmed = text.trim();
                            if trimmed == "__ready__" || trimmed == "__offer_ready__" {
                                continue;
                            }
                        }
                    },
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
        send_host_frame(
            host_transport.as_ref(),
            HostFrame::Hello {
                subscription: hello.subscription_id.0,
                max_seq: hello.max_seq.0,
                config: sync_config_to_wire(&hello.config),
            },
        );
        send_host_frame(
            host_transport.as_ref(),
            HostFrame::Grid {
                rows: rows as u32,
                cols: cols as u32,
            },
        );
        let mut cache = TransmitterCache::new();
        assert!(transmit_initial_snapshots(
            &host_transport,
            &mut synchronizer,
            &mut cache,
            subscription
        ));

        let mut saw_prompt = false;
        let mut view = ClientGrid::new(rows, cols);
        for _ in 0..20 {
            match client_transport.recv(StdDuration::from_millis(200)) {
                Ok(message) => match message.payload {
                    Payload::Binary(bytes) => {
                        match protocol::decode_host_frame_binary(&bytes).expect("host frame") {
                            WireHostFrame::Snapshot { updates, .. }
                            | WireHostFrame::Delta { updates, .. } => {
                                for update in &updates {
                                    view.apply_update(update);
                                }
                                if view.contains_row(prompt_trimmed) {
                                    saw_prompt = true;
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    Payload::Text(text) => {
                        let trimmed = text.trim();
                        if trimmed == "__ready__" || trimmed == "__offer_ready__" {
                            continue;
                        }
                    }
                },
                Err(TransportError::Timeout) => {}
                Err(err) => panic!("transport error: {err}"),
            }
        }

        assert!(saw_prompt, "snapshot did not include prompt row");
    }
}

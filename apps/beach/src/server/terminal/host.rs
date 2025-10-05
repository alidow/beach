use crate::cache::Seq;
use crate::cache::terminal::{PackedCell, StyleId, TerminalGrid, unpack_cell};
use crate::client::terminal::{ClientError, TerminalClient};
use crate::mcp::{
    McpConfig,
    bridge::spawn_webrtc_bridge,
    default_socket_path as mcp_default_socket_path,
    registry::{
        RegistryGuard as McpRegistryGuard, TerminalSession as McpTerminalSession,
        global_registry as mcp_global_registry,
    },
    server::{McpServer, McpServerHandle},
};
use crate::model::terminal::diff::{CacheUpdate, HistoryTrim, RowSnapshot, StyleDefinition};
use crate::protocol::terminal::bootstrap;
use crate::protocol::{
    self, ClientFrame as WireClientFrame, CursorFrame, FEATURE_CURSOR_SYNC, HostFrame,
    Lane as WireLane, LaneBudgetFrame as WireLaneBudget, SyncConfigFrame as WireSyncConfig,
    Update as WireUpdate, ViewportCommand,
};
use crate::server::terminal::{
    AlacrittyEmulator, Command as PtyCommand, LocalEcho, PtyProcess, PtyWriter, SpawnConfig,
    TerminalEmulator, TerminalRuntime,
};
use crate::session::terminal::authorization::{JoinAuthorizationMetadata, JoinAuthorizer};
use crate::session::terminal::tty::{HostInputGate, RawModeGuard};
use crate::session::{HostSession, SessionConfig, SessionHandle, SessionManager};
use crate::sync::terminal::{TerminalDeltaStream, TerminalSync};
use crate::sync::{LaneBudget, PriorityLane, ServerSynchronizer, SubscriptionId, SyncConfig};
use crate::telemetry::logging as logctl;
use crate::telemetry::{self, PerfGuard};
use crate::terminal::app::{kind_label, summarize_offers};
use crate::terminal::cli::{BootstrapOutput, HostArgs};
use crate::terminal::config::cursor_sync_enabled;
use crate::terminal::error::CliError;
use crate::terminal::negotiation::{NegotiatedSingle, NegotiatedTransport, negotiate_transport};
use crate::transport as transport_mod;
use crate::transport::{
    Payload, Transport, TransportError, TransportId, TransportKind, TransportMessage,
};
use anyhow::Result as AnyResult;
use crossterm::terminal::size as terminal_size;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::{self, Write as _};
use std::io::{self, IsTerminal, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{interval, sleep, timeout};
use tracing::{Level, debug, error, info, trace, warn};
use transport_mod::webrtc::{OffererAcceptedTransport, OffererSupervisor};

pub(crate) const MCP_CHANNEL_LABEL: &str = "mcp-jsonrpc";
pub(crate) const MCP_CHANNEL_TIMEOUT: Duration = Duration::from_secs(30);

pub async fn run(base_url: &str, args: HostArgs) -> Result<(), CliError> {
    let manager = SessionManager::new(SessionConfig::new(base_url)?)?;
    let cursor_sync = cursor_sync_enabled();
    let normalized_base = manager.config().base_url().to_string();
    let bootstrap_output = args.bootstrap_output;
    let bootstrap_mode = bootstrap_output == BootstrapOutput::Json;
    configure_bootstrap_signal_handling(bootstrap_mode);
    let local_preview_requested = args.local_preview;
    let local_preview_enabled = local_preview_requested && !bootstrap_mode;
    if local_preview_requested && !local_preview_enabled {
        warn!("local preview disabled when bootstrap output is active");
    }
    let interactive = !bootstrap_mode && io::stdin().is_terminal() && io::stdout().is_terminal();
    let raw_guard = RawModeGuard::new(interactive);

    let input_gate = if interactive {
        Some(Arc::new(HostInputGate::new()))
    } else {
        None
    };

    let allow_all_clients = args.allow_all_clients || !interactive || bootstrap_mode;
    if allow_all_clients {
        debug!("client authorization prompt disabled (allow-all mode)");
    }
    let authorizer = Arc::new(if allow_all_clients {
        JoinAuthorizer::allow_all()
    } else {
        let gate = input_gate
            .as_ref()
            .expect("interactive input gate must be present for prompts");
        JoinAuthorizer::interactive(Arc::clone(gate))
    });

    let hosted = manager.host().await?;
    let session_id = hosted.session_id().to_string();
    info!(session_id = %session_id, "session registered");
    // In bootstrap mode, respect the --wait flag to control whether we wait for peer
    // (this allows SSH bootstrap to output JSON immediately without waiting)
    let wait_for_peer = args.wait;
    let command = resolve_launch_command(&args)?;
    let command_display = display_cmd(&command);
    if bootstrap_mode {
        bootstrap::emit_bootstrap_handshake(
            &hosted,
            &normalized_base,
            TransportKind::WebRtc,
            &command,
            wait_for_peer,
            args.mcp,
        )?;
        // Flush stdout to ensure JSON is written before the shell starts
        std::io::stdout().flush().ok();
    } else {
        print_host_banner(&hosted, &normalized_base, TransportKind::WebRtc, args.mcp);
    }

    let session_handle = hosted.handle().clone();
    let join_code = hosted.join_code().to_string();
    let transports: Arc<Mutex<Vec<Arc<SharedTransport>>>> = Arc::new(Mutex::new(Vec::new()));

    if wait_for_peer {
        info!(session_id = %session_id, "waiting for WebRTC transport");
    } else {
        info!(session_id = %session_id, "negotiating transport in background");
    }

    let (spawn_config, grid) = build_spawn_config(&command)?;
    let sync_config = SyncConfig::default();
    let timeline = Arc::new(TimelineDeltaStream::new());
    let delta_stream: Arc<dyn TerminalDeltaStream> = timeline.clone();
    let terminal_sync = Arc::new(TerminalSync::new(
        grid.clone(),
        delta_stream,
        sync_config.clone(),
    ));
    let (backfill_tx, backfill_rx) = mpsc::unbounded_channel();

    let emulator = Box::new(AlacrittyEmulator::new(&grid, cursor_sync));
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

    let mut mcp_task: Option<JoinHandle<()>> = None;
    let mut mcp_handle: Option<McpServerHandle> = None;
    let mcp_bridges: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));
    let _mcp_guard: Option<McpRegistryGuard> = if args.mcp {
        let session = McpTerminalSession::new(
            session_id.clone(),
            terminal_sync.clone(),
            writer.clone(),
            process_handle.clone(),
        );
        let guard = mcp_global_registry().register_terminal(session);
        let resolved_socket = if args.mcp_stdio {
            None
        } else {
            Some(
                args.mcp_socket
                    .clone()
                    .unwrap_or_else(|| mcp_default_socket_path(&session_id)),
            )
        };
        let mut config = McpConfig::default();
        config.socket = resolved_socket.clone();
        config.use_stdio = args.mcp_stdio;
        config.allow_write = args.mcp_allow_write;
        config.read_only = !args.mcp_allow_write;
        config.session_filter = Some(vec![session_id.clone()]);
        let server = McpServer::new(config);
        let handle = server.handle();
        mcp_handle = Some(handle.clone());
        mcp_task = Some(tokio::spawn(async move {
            if let Err(err) = server.run().await {
                warn!(error = %err, "mcp server terminated");
            }
        }));
        if let Some(path) = resolved_socket {
            if !bootstrap_mode {
                println!("🔌 MCP socket listening at {}", path.display());
            } else {
                info!(socket = %path.display(), "mcp socket ready");
            }
        }
        Some(guard)
    } else {
        None
    };

    info!(session_id = %session_id, "host ready");

    let input_handles: Arc<Mutex<Vec<thread::JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));
    let mut forward_transports: Vec<(Arc<dyn Transport>, Option<Arc<TransportSupervisor>>)> =
        Vec::new();

    let mut local_preview_task: Option<tokio::task::JoinHandle<()>> = None;
    let local_server_transport: Arc<Mutex<Option<Arc<dyn Transport>>>> = Arc::new(Mutex::new(None));

    if local_preview_enabled {
        let pair = transport_mod::TransportPair::new(TransportKind::Ipc);
        let local_client_transport: Arc<dyn Transport> = Arc::from(pair.client);
        let local_server: Arc<dyn Transport> = Arc::from(pair.server);

        {
            let handle = spawn_input_listener(
                local_server.clone(),
                writer.clone(),
                process_handle.clone(),
                emulator_handle.clone(),
                grid.clone(),
                backfill_tx.clone(),
                None,
                None,
            );
            input_handles.lock().unwrap().push(handle);
        }

        local_preview_task = Some(tokio::task::spawn_blocking(move || {
            let client = TerminalClient::new(local_client_transport).with_predictive_input(true);
            match client.run() {
                Ok(()) | Err(ClientError::Shutdown) => {}
                Err(err) => eprintln!("⚠️  preview client error: {err}"),
            }
        }));

        forward_transports.push((local_server.clone(), None));
        {
            let mut guard = local_server_transport.lock().unwrap();
            *guard = Some(local_server);
        }
        debug!(session_id = %session_id, "local preview transport attached");
    }

    let resize_monitor = if interactive {
        let gate = input_gate
            .as_ref()
            .expect("interactive input gate must exist")
            .clone();
        let handle = spawn_local_stdin_forwarder(writer.clone(), local_echo.clone(), gate);
        input_handles.lock().unwrap().push(handle);

        let running = Arc::new(AtomicBool::new(true));
        let resize_handle = spawn_local_resize_monitor(
            running.clone(),
            process_handle.clone(),
            emulator_handle.clone(),
            grid.clone(),
            transports.clone(),
            Arc::clone(&local_server_transport),
        );
        input_handles.lock().unwrap().push(resize_handle);

        Some(running)
    } else {
        None
    };

    let (forwarder_cmd_tx, forwarder_cmd_rx) = mpsc::unbounded_channel();

    let (first_ready_tx, first_ready_rx) = if wait_for_peer {
        let (tx, rx) = oneshot::channel();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    let accept_task = spawn_webrtc_acceptor(
        session_id.clone(),
        session_handle.clone(),
        Some(join_code.clone()),
        writer.clone(),
        process_handle.clone(),
        emulator_handle.clone(),
        grid.clone(),
        backfill_tx.clone(),
        input_handles.clone(),
        forwarder_cmd_tx.clone(),
        transports.clone(),
        Arc::clone(&authorizer),
        mcp_handle.clone(),
        Arc::clone(&mcp_bridges),
        first_ready_tx,
    );

    if wait_for_peer {
        if let Some(rx) = first_ready_rx {
            rx.await.map_err(|_| {
                CliError::TransportNegotiation("webrtc transport was not ready".into())
            })?;
        }
    }

    let updates_task = spawn_update_forwarder(
        forward_transports,
        updates,
        timeline.clone(),
        terminal_sync.clone(),
        sync_config.clone(),
        backfill_rx,
        forwarder_cmd_rx,
        Some(forwarder_cmd_tx.clone()),
        transports.clone(),
        cursor_sync,
    );

    runtime
        .wait()
        .await
        .map_err(|err| CliError::Runtime(err.to_string()))?;

    // Restore cooked mode before we print shutdown banners so the host shell
    // redraws cleanly (mirrors the legacy apps/beach behaviour).
    drop(raw_guard);

    if let Some(flag) = &resize_monitor {
        flag.store(false, Ordering::SeqCst);
    }

    accept_task.abort();
    let _ = accept_task.await;

    let transports_snapshot: Vec<Arc<SharedTransport>> = {
        let guard = transports.lock().unwrap();
        guard.iter().cloned().collect()
    };

    for shared in transports_snapshot {
        let transport: Arc<dyn Transport> = shared;
        let _ = send_host_frame(&transport, HostFrame::Shutdown);
    }
    let local_server_snapshot = local_server_transport.lock().unwrap().clone();
    if let Some(server) = local_server_snapshot {
        let _ = send_host_frame(&server, HostFrame::Shutdown);
    }

    if let Err(err) = updates_task.await {
        eprintln!("⚠️  update forwarder ended unexpectedly: {err}");
    }

    if let Some(handle) = local_preview_task {
        let _ = handle.await;
    }

    let mut guard = input_handles.lock().unwrap();
    for handle in guard.drain(..) {
        handle.join().ok();
    }

    let bridge_handles: Vec<JoinHandle<()>> = {
        let mut guard = mcp_bridges.lock().unwrap();
        guard.drain(..).collect()
    };
    for handle in bridge_handles {
        handle.abort();
        let _ = handle.await;
    }

    if let Some(handle) = mcp_task {
        handle.abort();
        let _ = handle.await;
    }

    if !bootstrap_mode {
        println!("\n✅ command '{}' completed", command_display);
    }
    info!(session_id = %session_id, "host command completed");
    Ok(())
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

fn configure_bootstrap_signal_handling(bootstrap_mode: bool) {
    #[cfg(unix)]
    {
        if bootstrap_mode {
            unsafe {
                let result = libc::signal(libc::SIGHUP, libc::SIG_IGN);
                if result == libc::SIG_ERR {
                    warn!("failed to ignore SIGHUP for bootstrap mode");
                } else {
                    debug!("ignoring SIGHUP so the host survives ssh disconnects");
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = bootstrap_mode;
    }
}

fn print_host_banner(
    session: &HostSession,
    base: &str,
    selected: TransportKind,
    mcp_enabled: bool,
) {
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
    println!("  active     : {}", kind_label(selected));

    if mcp_enabled {
        println!(
            "  mcp bridge  : beach --session-server {} join {} --passcode {} --mcp",
            base,
            handle.session_id,
            session.join_code()
        );
    }
    println!();
    println!("🌊 Launching host process... type 'exit' to end the session.\n");
}

#[derive(Clone)]
struct HeartbeatPublisher {
    transport: Arc<dyn Transport>,
    supervisor: Option<Arc<TransportSupervisor>>,
}

impl HeartbeatPublisher {
    fn new(transport: Arc<dyn Transport>, supervisor: Option<Arc<TransportSupervisor>>) -> Self {
        Self {
            transport,
            supervisor,
        }
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
                let frame = HostFrame::Heartbeat {
                    seq: count as u64,
                    timestamp_ms: now as u64,
                };

                if let Err(err) = send_host_frame(&self.transport, frame) {
                    debug!(
                        target = "transport_mod::heartbeat",
                        transport_id = self.transport.id().0,
                        transport = ?self.transport.kind(),
                        error = %err,
                        "heartbeat send failed; scheduling reconnect"
                    );
                    if let Some(supervisor) = &self.supervisor {
                        supervisor.schedule_reconnect();
                        sleep(interval).await;
                        continue;
                    } else {
                        warn!(
                            target = "transport_mod::heartbeat",
                            transport_id = self.transport.id().0,
                            transport = ?self.transport.kind(),
                            error = %err,
                            "heartbeat publisher stopping after failed send"
                        );
                        break;
                    }
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
    if let Ok((cols, rows)) = terminal_size() {
        if cols > 0 && rows > 0 {
            return (cols.max(20), rows.max(10));
        }
    }

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

const MAX_BACKFILL_ROWS_PER_REQUEST: u32 = 256;
const SERVER_BACKFILL_CHUNK_ROWS: u32 = 64;
const MAX_TRANSPORT_FRAME_BYTES: usize = 48 * 1024;
const MAX_UPDATES_PER_FRAME: usize = 64;
const MAX_PTY_COLS: u16 = 200;
const MAX_PTY_ROWS: u16 = 200;
const SERVER_BACKFILL_THROTTLE: Duration = Duration::from_millis(50);

struct BackfillChunk {
    updates: Vec<CacheUpdate>,
    attempted: u32,
    delivered: u32,
}

#[derive(Clone, Debug)]
struct BackfillCommand {
    transport_id: TransportId,
    subscription: u64,
    request_id: u64,
    start_row: u64,
    count: u32,
}

#[derive(Debug)]
struct BackfillJob {
    subscription: u64,
    request_id: u64,
    next_row: u64,
    end_row: u64,
}

fn collect_backfill_chunk(grid: &TerminalGrid, start_row: u64, max_rows: u32) -> BackfillChunk {
    if max_rows == 0 {
        return BackfillChunk {
            updates: Vec::new(),
            attempted: 0,
            delivered: 0,
        };
    }

    let cols = grid.cols();
    if cols == 0 {
        return BackfillChunk {
            updates: Vec::new(),
            attempted: max_rows,
            delivered: 0,
        };
    }

    let mut updates = Vec::new();
    let mut buffer: Vec<u64> = vec![0; cols];
    let mut style_ids: HashSet<StyleId> = HashSet::new();
    let mut delivered = 0u32;

    let base_offset = grid.row_offset();
    let mut effective_start = start_row;
    if start_row < base_offset {
        let diff = base_offset - start_row;
        if let (Ok(start), Ok(count)) = (usize::try_from(start_row), usize::try_from(diff)) {
            updates.push(CacheUpdate::Trim(HistoryTrim::new(start, count)));
            trace!(
                target = "sync::backfill",
                start_row, base_offset, count, "emitting trim for backfill"
            );
        } else {
            trace!(
                target = "sync::backfill",
                start_row, base_offset, diff, "trim conversion overflow"
            );
        }
        effective_start = base_offset;
    }

    trace!(
        target = "sync::backfill",
        start_row, effective_start, max_rows, base_offset, cols, "collecting backfill chunk"
    );

    let default_cell = TerminalGrid::pack_char_with_style(' ', StyleId::DEFAULT);
    let first_id = grid.first_row_id();
    let last_id = grid.last_row_id();
    trace!(
        target = "sync::backfill",
        start_row,
        effective_start,
        max_rows,
        base_offset,
        cols,
        first_id,
        last_id,
        total_rows = grid.rows(),
        "collecting backfill chunk"
    );

    for offset in 0..max_rows as u64 {
        let absolute = effective_start.saturating_add(offset);
        let Some(index) = grid.index_of_row(absolute) else {
            trace!(target = "sync::backfill", absolute, "row missing from grid");
            continue;
        };

        if grid.snapshot_row_into(index, &mut buffer[..cols]).is_err() {
            continue;
        }

        if tracing::enabled!(Level::TRACE) && offset < 4 {
            let preview: String = buffer
                .iter()
                .map(|cell| unpack_cell(PackedCell::from(*cell)).0)
                .collect();
            trace!(
                target = "sync::backfill",
                row = absolute,
                text = %preview.trim_end_matches(' ')
            );
        }

        let mut max_seq = 0;
        let mut packed_cells: Vec<PackedCell> = Vec::with_capacity(cols);
        for col in 0..cols {
            if let Some(snapshot) = grid.get_cell_relaxed(index, col) {
                max_seq = max_seq.max(snapshot.seq);
            }
            let packed = PackedCell::from(buffer[col]);
            let (_, style_id) = unpack_cell(packed);
            style_ids.insert(style_id);
            packed_cells.push(packed);
        }

        if max_seq == 0
            && packed_cells
                .iter()
                .all(|cell| u64::from(*cell) == u64::from(default_cell))
        {
            trace!(
                target = "sync::backfill",
                row = absolute,
                "skipping default row with no seq"
            );
            continue;
        }

        updates.push(CacheUpdate::Row(RowSnapshot::new(
            absolute as usize,
            max_seq,
            packed_cells,
        )));
        delivered = delivered.saturating_add(1);
    }

    if delivered > 0 {
        let style_table = grid.style_table.clone();
        for style_id in style_ids {
            if let Some(style) = style_table.get(style_id) {
                updates.push(CacheUpdate::Style(StyleDefinition::new(
                    style_id,
                    effective_start,
                    style,
                )));
            }
        }
    }

    BackfillChunk {
        updates,
        attempted: max_rows,
        delivered,
    }
}

fn host_frame_label(frame: &HostFrame) -> &'static str {
    match frame {
        HostFrame::Heartbeat { .. } => "heartbeat",
        HostFrame::Hello { .. } => "hello",
        HostFrame::Grid { .. } => "grid",
        HostFrame::Snapshot { .. } => "snapshot",
        HostFrame::SnapshotComplete { .. } => "snapshot_complete",
        HostFrame::Delta { .. } => "delta",
        HostFrame::HistoryBackfill { .. } => "history_backfill",
        HostFrame::Cursor { .. } => "cursor",
        HostFrame::InputAck { .. } => "input_ack",
        HostFrame::Shutdown => "shutdown",
    }
}

fn client_frame_label(frame: &WireClientFrame) -> &'static str {
    match frame {
        WireClientFrame::Input { .. } => "input",
        WireClientFrame::Resize { .. } => "resize",
        WireClientFrame::RequestBackfill { .. } => "request_backfill",
        WireClientFrame::ViewportCommand { .. } => "viewport_command",
        WireClientFrame::Unknown => "unknown",
    }
}

fn send_host_frame(transport: &Arc<dyn Transport>, frame: HostFrame) -> Result<(), TransportError> {
    let encode_start = Instant::now();
    let frame_label = host_frame_label(&frame);
    if tracing::enabled!(Level::TRACE) {
        match &frame {
            HostFrame::Delta {
                updates, watermark, ..
            } => {
                let trim_count = updates
                    .iter()
                    .filter(|update| matches!(update, crate::protocol::Update::Trim { .. }))
                    .count();
                if trim_count > 0 {
                    trace!(
                        target = "sync::transport",
                        frame = frame_label,
                        trims = trim_count,
                        watermark,
                        "sending delta with trims"
                    );
                }
            }
            HostFrame::HistoryBackfill {
                updates,
                request_id,
                start_row,
                count,
                more,
                ..
            } => {
                let trim_count = updates
                    .iter()
                    .filter(|update| matches!(update, crate::protocol::Update::Trim { .. }))
                    .count();
                if trim_count > 0 {
                    trace!(
                        target = "sync::transport",
                        frame = frame_label,
                        trims = trim_count,
                        request_id,
                        start_row,
                        count,
                        more,
                        "sending history backfill with trims"
                    );
                }
            }
            _ => {}
        }
    }
    let bytes = protocol::encode_host_frame_binary(&frame);
    let elapsed = encode_start.elapsed();
    match &frame {
        HostFrame::Snapshot { .. } => telemetry::record_duration("sync_encode_snapshot", elapsed),
        HostFrame::Delta { .. } => telemetry::record_duration("sync_encode_delta", elapsed),
        _ => telemetry::record_duration("sync_encode_frame", elapsed),
    }
    match transport.send_bytes(&bytes) {
        Ok(sequence) => {
            if tracing::enabled!(Level::TRACE) {
                trace!(
                    target = "sync::transport",
                    transport_id = transport.id().0,
                    transport = ?transport.kind(),
                    frame = frame_label,
                    payload_len = bytes.len(),
                    sequence,
                    "host frame sent"
                );
            }
            Ok(())
        }
        Err(err) => {
            debug!(
                target = "sync::transport",
                transport_id = transport.id().0,
                transport = ?transport.kind(),
                frame = frame_label,
                error = %err,
                "failed to send host frame"
            );
            Err(err)
        }
    }
}

fn send_snapshot_frames_chunked(
    transport: &Arc<dyn Transport>,
    subscription: SubscriptionId,
    lane: PriorityLane,
    watermark: Seq,
    has_more: bool,
    batch: PreparedUpdateBatch,
) -> Result<(), TransportError> {
    let wire_lane = map_lane(lane);
    send_chunked_updates(
        transport,
        batch,
        has_more,
        |chunk_updates, chunk_has_more, cursor| HostFrame::Snapshot {
            subscription: subscription.0,
            lane: wire_lane,
            watermark,
            has_more: chunk_has_more,
            updates: chunk_updates,
            cursor,
        },
    )
}

fn send_delta_frames_chunked(
    transport: &Arc<dyn Transport>,
    subscription: SubscriptionId,
    watermark: Seq,
    has_more: bool,
    batch: PreparedUpdateBatch,
) -> Result<(), TransportError> {
    send_chunked_updates(
        transport,
        batch,
        has_more,
        |chunk_updates, chunk_has_more, cursor| HostFrame::Delta {
            subscription: subscription.0,
            watermark,
            has_more: chunk_has_more,
            updates: chunk_updates,
            cursor,
        },
    )
}

fn send_chunked_updates<F>(
    transport: &Arc<dyn Transport>,
    batch: PreparedUpdateBatch,
    final_has_more: bool,
    mut build_frame: F,
) -> Result<(), TransportError>
where
    F: FnMut(Vec<WireUpdate>, bool, Option<CursorFrame>) -> HostFrame,
{
    if batch.updates.is_empty() {
        let frame = build_frame(Vec::new(), final_has_more, batch.cursor);
        return send_host_frame(transport, frame);
    }

    let mut remaining: VecDeque<WireUpdate> = batch.updates.into();
    let mut chunk: Vec<WireUpdate> = Vec::new();
    let mut cursor_pending = batch.cursor;

    while let Some(update) = remaining.pop_front() {
        chunk.push(update);
        loop {
            let more_updates_pending = !remaining.is_empty();
            let chunk_has_more = more_updates_pending || final_has_more;
            let cursor_frame = cursor_pending.clone();
            let frame = build_frame(chunk.clone(), chunk_has_more, cursor_frame.clone());
            let encoded_len = protocol::encode_host_frame_binary(&frame).len();

            if encoded_len > MAX_TRANSPORT_FRAME_BYTES && chunk.len() > 1 {
                let overflow = chunk.pop().expect("chunk entry exists");
                let chunk_cursor = cursor_pending.clone();
                let chunk_frame = build_frame(chunk.clone(), true, chunk_cursor.clone());
                let chunk_len = protocol::encode_host_frame_binary(&chunk_frame).len();
                trace!(
                    target = "sync::transport",
                    chunk_updates = chunk.len(),
                    encoded_len = chunk_len,
                    limit = MAX_TRANSPORT_FRAME_BYTES,
                    "sending chunked host frame"
                );
                send_host_frame(transport, chunk_frame)?;
                if chunk_cursor.is_some() {
                    cursor_pending = None;
                }
                chunk.clear();
                chunk.push(overflow);
                continue;
            }

            if encoded_len > MAX_TRANSPORT_FRAME_BYTES {
                trace!(
                    target = "sync::transport",
                    chunk_updates = chunk.len(),
                    encoded_len,
                    limit = MAX_TRANSPORT_FRAME_BYTES,
                    "sending oversized single-update frame"
                );
                send_host_frame(transport, frame)?;
                if cursor_frame.is_some() {
                    cursor_pending = None;
                }
                chunk.clear();
                break;
            }

            if chunk.len() >= MAX_UPDATES_PER_FRAME {
                trace!(
                    target = "sync::transport",
                    chunk_updates = chunk.len(),
                    encoded_len,
                    limit = MAX_TRANSPORT_FRAME_BYTES,
                    "sending chunked host frame"
                );
                send_host_frame(transport, frame)?;
                if cursor_frame.is_some() {
                    cursor_pending = None;
                }
                chunk.clear();
                break;
            }

            if !more_updates_pending {
                let final_cursor = cursor_pending.clone();
                let final_frame = build_frame(chunk.clone(), final_has_more, final_cursor.clone());
                let final_len = protocol::encode_host_frame_binary(&final_frame).len();
                trace!(
                    target = "sync::transport",
                    chunk_updates = chunk.len(),
                    encoded_len = final_len,
                    limit = MAX_TRANSPORT_FRAME_BYTES,
                    "sending final chunked host frame"
                );
                send_host_frame(transport, final_frame)?;
                if final_cursor.is_some() {
                    cursor_pending = None;
                }
                chunk.clear();
                break;
            }

            break;
        }

        if chunk.is_empty() {
            continue;
        }
    }

    if !chunk.is_empty() {
        let final_cursor = cursor_pending.clone();
        let final_frame = build_frame(chunk.clone(), final_has_more, final_cursor.clone());
        let encoded_len = protocol::encode_host_frame_binary(&final_frame).len();
        trace!(
            target = "sync::transport",
            chunk_updates = chunk.len(),
            encoded_len,
            limit = MAX_TRANSPORT_FRAME_BYTES,
            "sending trailing chunked host frame"
        );
        send_host_frame(transport, final_frame)?;
        if final_cursor.is_some() {}
    }

    Ok(())
}

fn handle_viewport_command(
    command: ViewportCommand,
    writer: &PtyWriter,
    transport_id: u64,
    transport_kind: &TransportKind,
    grid: &Arc<TerminalGrid>,
    forwarder_tx: &Option<UnboundedSender<ForwarderCommand>>,
) -> AnyResult<()> {
    match command {
        ViewportCommand::Clear => {
            writer.write(&[0x0c])?;
            grid.clear_viewport();
            debug!(
                target = "sync::incoming",
                transport_id,
                transport = ?transport_kind,
                "viewport clear command applied"
            );
            if let Some(tx) = forwarder_tx {
                if tx.send(ForwarderCommand::ViewportRefresh).is_err() {
                    trace!(
                        target = "sync::incoming",
                        transport_id = transport_id,
                        "viewport refresh send failed (receiver closed)"
                    );
                }
            }
        }
    }
    Ok(())
}

struct SharedTransport {
    inner: RwLock<Arc<dyn Transport>>,
}

impl SharedTransport {
    fn new(initial: Arc<dyn Transport>) -> Self {
        Self {
            inner: RwLock::new(initial),
        }
    }

    fn swap(&self, next: Arc<dyn Transport>) {
        let mut guard = self.inner.write().expect("shared transport poisoned");
        *guard = next;
    }

    fn current(&self) -> Arc<dyn Transport> {
        self.inner
            .read()
            .expect("shared transport poisoned")
            .clone()
    }
}

impl fmt::Debug for SharedTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let current = self.current();
        f.debug_struct("SharedTransport")
            .field("transport_id", &current.id())
            .field("transport_kind", &current.kind())
            .finish()
    }
}

impl Transport for SharedTransport {
    fn kind(&self) -> TransportKind {
        self.current().kind()
    }

    fn id(&self) -> TransportId {
        self.current().id()
    }

    fn peer(&self) -> TransportId {
        self.current().peer()
    }

    fn send(&self, message: TransportMessage) -> Result<(), TransportError> {
        self.current().send(message)
    }

    fn send_text(&self, text: &str) -> Result<u64, TransportError> {
        self.current().send_text(text)
    }

    fn send_bytes(&self, bytes: &[u8]) -> Result<u64, TransportError> {
        self.current().send_bytes(bytes)
    }

    fn recv(&self, timeout: Duration) -> Result<TransportMessage, TransportError> {
        self.current().recv(timeout)
    }

    fn try_recv(&self) -> Result<Option<TransportMessage>, TransportError> {
        self.current().try_recv()
    }
}

#[derive(Clone)]
struct TransportSupervisor {
    shared: Arc<SharedTransport>,
    session_handle: SessionHandle,
    passphrase: Option<String>,
    reconnecting: Arc<AsyncMutex<bool>>,
}

impl TransportSupervisor {
    fn new(
        shared: Arc<SharedTransport>,
        session_handle: SessionHandle,
        passphrase: Option<String>,
    ) -> Self {
        Self {
            shared,
            session_handle,
            passphrase,
            reconnecting: Arc::new(AsyncMutex::new(false)),
        }
    }

    fn schedule_reconnect(&self) {
        let this = self.clone();
        tokio::spawn(async move {
            let mut guard = this.reconnecting.lock().await;
            if *guard {
                return;
            }
            *guard = true;
            drop(guard);

            const MAX_ATTEMPTS: usize = 5;
            let mut delay = Duration::from_millis(250);
            for attempt in 1..=MAX_ATTEMPTS {
                match negotiate_transport(
                    &this.session_handle,
                    this.passphrase.as_deref(),
                    None,
                    false,
                )
                .await
                {
                    Ok(NegotiatedTransport::Single(NegotiatedSingle {
                        transport: new_transport,
                        ..
                    })) => {
                        let kind = new_transport.kind();
                        let id = new_transport.id().0;
                        this.shared.swap(new_transport);
                        info!(
                            target = "transport_mod::failover",
                            ?kind,
                            transport_id = id,
                            attempt,
                            "transport failover completed"
                        );
                        break;
                    }
                    Ok(NegotiatedTransport::WebRtcOfferer { connection, .. }) => {
                        let transport = connection.transport();
                        let kind = transport.kind();
                        let id = transport.id().0;
                        this.shared.swap(transport);
                        info!(
                            target = "transport_mod::failover",
                            ?kind,
                            transport_id = id,
                            attempt,
                            "transport failover completed (offerer)"
                        );
                        break;
                    }
                    Err(err) => {
                        warn!(
                            target = "transport_mod::failover",
                            attempt,
                            error = %err,
                            "transport failover attempt failed"
                        );
                        if attempt == MAX_ATTEMPTS {
                            error!(
                                target = "transport_mod::failover",
                                "exhausted transport failover attempts"
                            );
                            break;
                        }
                        sleep(delay).await;
                        delay = (delay * 2).min(Duration::from_secs(5));
                    }
                }
            }

            let mut guard = this.reconnecting.lock().await;
            *guard = false;
        });
    }
}

fn map_lane(lane: PriorityLane) -> WireLane {
    match lane {
        PriorityLane::Foreground => WireLane::Foreground,
        PriorityLane::Recent => WireLane::Recent,
        PriorityLane::History => WireLane::History,
    }
}

fn spawn_input_listener(
    transport: Arc<dyn Transport>,
    writer: PtyWriter,
    process: Arc<PtyProcess>,
    emulator: Arc<Mutex<Box<dyn TerminalEmulator + Send>>>,
    grid: Arc<TerminalGrid>,
    backfill_tx: UnboundedSender<BackfillCommand>,
    forwarder_tx: Option<UnboundedSender<ForwarderCommand>>,
    gate: Option<Arc<HostInputGate>>,
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
        let mut channel_closed = false;
        let mut fatal_error = false;
        loop {
            if let Some(g) = &gate {
                g.wait_until_resumed();
            }
            match transport.recv(Duration::from_millis(250)) {
                Ok(message) => {
                    let transport_sequence = message.sequence;
                    match message.payload {
                        Payload::Binary(bytes) => {
                            match protocol::decode_client_frame_binary(&bytes) {
                                Ok(frame) => {
                                    if tracing::enabled!(Level::TRACE) {
                                        trace!(
                                            target = "sync::incoming",
                                            transport_id,
                                            transport = ?transport_kind,
                                            transport_sequence,
                                            frame = client_frame_label(&frame),
                                            payload_len = bytes.len(),
                                            "received client frame"
                                        );
                                    }
                                    match frame {
                                        WireClientFrame::Input { seq, data } => {
                                            if let Some(g) = &gate {
                                                g.wait_until_resumed();
                                            }
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
                                            let _ = send_host_frame(
                                                &transport,
                                                HostFrame::InputAck { seq },
                                            );
                                            debug!(
                                                target = "sync::incoming",
                                                transport_id,
                                                transport = ?transport_kind,
                                                seq,
                                                "input applied and acked"
                                            );
                                        }
                                        WireClientFrame::Resize { cols, rows } => {
                                            let clamped_cols = cols.min(MAX_PTY_COLS);
                                            let clamped_rows = rows.min(MAX_PTY_ROWS);
                                            if cols != clamped_cols || rows != clamped_rows {
                                                trace!(
                                                    target = "sync::incoming",
                                                    transport_id,
                                                    transport = ?transport_kind,
                                                    requested_cols = cols,
                                                    requested_rows = rows,
                                                    clamped_cols,
                                                    clamped_rows,
                                                    "clamped resize request"
                                                );
                                            }
                                            if let Err(err) =
                                                process.resize(clamped_cols, clamped_rows)
                                            {
                                                warn!(
                                                    target = "sync::incoming",
                                                    transport_id,
                                                    transport = ?transport_kind,
                                                    error = %err,
                                                    cols = clamped_cols,
                                                    rows = clamped_rows,
                                                    "pty resize failed"
                                                );
                                            }
                                            if let Ok(mut guard) = emulator.lock() {
                                                guard.resize(
                                                    clamped_rows as usize,
                                                    clamped_cols as usize,
                                                );
                                            }
                                            grid.set_viewport_size(
                                                clamped_rows as usize,
                                                clamped_cols as usize,
                                            );
                                            let history_rows = grid.rows();
                                            let _ = send_host_frame(
                                                &transport,
                                                HostFrame::Grid {
                                                    cols: clamped_cols as u32,
                                                    history_rows: history_rows as u32,
                                                    base_row: grid.row_offset(),
                                                    viewport_rows: Some(clamped_rows as u32),
                                                },
                                            );
                                            debug!(
                                                target = "sync::incoming",
                                                transport_id,
                                                transport = ?transport_kind,
                                                cols = clamped_cols,
                                                rows = clamped_rows,
                                                "processed resize request"
                                            );
                                        }
                                        WireClientFrame::ViewportCommand { command } => {
                                            if let Err(err) = handle_viewport_command(
                                                command,
                                                &writer,
                                                transport_id,
                                                &transport_kind,
                                                &grid,
                                                &forwarder_tx,
                                            ) {
                                                warn!(
                                                    target = "sync::incoming",
                                                    transport_id,
                                                    transport = ?transport_kind,
                                                    error = %err,
                                                    command = ?command,
                                                    "viewport command failed"
                                                );
                                            }
                                        }
                                        WireClientFrame::RequestBackfill {
                                            subscription,
                                            request_id,
                                            start_row,
                                            count,
                                        } => {
                                            let capped = count.min(MAX_BACKFILL_ROWS_PER_REQUEST);
                                            if capped == 0 {
                                                continue;
                                            }
                                            if let Err(err) = backfill_tx.send(BackfillCommand {
                                                transport_id: transport.id(),
                                                subscription,
                                                request_id,
                                                start_row,
                                                count: capped,
                                            }) {
                                                warn!(
                                                    target = "sync::incoming",
                                                    transport_id,
                                                    transport = ?transport_kind,
                                                    request_id,
                                                    error = %err,
                                                    "failed to enqueue backfill request"
                                                );
                                            } else {
                                                trace!(
                                                    target = "sync::incoming",
                                                    transport_id,
                                                    transport = ?transport_kind,
                                                    request_id,
                                                    start_row,
                                                    requested = count,
                                                    enqueued = capped,
                                                    "queued history backfill request"
                                                );
                                            }
                                        }
                                        WireClientFrame::Unknown => {}
                                    }
                                }
                                Err(err) => {
                                    warn!(
                                        target = "sync::incoming",
                                        transport_id,
                                        transport = ?transport_kind,
                                        transport_sequence,
                                        error = %err,
                                        "failed to decode client frame"
                                    );
                                }
                            }
                        }
                        Payload::Text(text) => {
                            let trimmed = text.trim();
                            if trimmed == "__ready__" || trimmed == "__offer_ready__" {
                                trace!(
                                    target = "sync::incoming",
                                    transport_id,
                                    transport = ?transport_kind,
                                    transport_sequence,
                                    "ignoring handshake sentinel"
                                );
                            } else {
                                debug!(
                                    target = "sync::incoming",
                                    transport_id,
                                    transport = ?transport_kind,
                                    transport_sequence,
                                    payload = %trimmed,
                                    "ignoring unexpected text payload"
                                );
                            }
                        }
                    }
                }
                Err(TransportError::Timeout) => continue,
                Err(TransportError::ChannelClosed) => {
                    channel_closed = true;
                    break;
                }
                Err(err) => {
                    warn!(
                        target = "sync::incoming",
                        transport_id,
                        transport = ?transport_kind,
                        error = %err,
                        "input listener error"
                    );
                    fatal_error = true;
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
        if channel_closed || fatal_error {
            if let Some(tx) = &forwarder_tx {
                let id = transport.id();
                if tx.send(ForwarderCommand::RemoveTransport { id }).is_err() {
                    trace!(
                        target = "sync::incoming",
                        transport_id,
                        transport = ?transport_kind,
                        "forwarder dropped remove transport command"
                    );
                } else {
                    debug!(
                        target = "sync::incoming",
                        transport_id,
                        transport = ?transport_kind,
                        "notified forwarder of transport removal"
                    );
                }
            }
        }
    })
}

fn spawn_local_stdin_forwarder(
    writer: PtyWriter,
    local_echo: Arc<LocalEcho>,
    gate: Arc<HostInputGate>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut stdin = io::stdin();
        let mut buffer = [0u8; 1024];
        loop {
            gate.wait_until_resumed();
            match stdin.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    let bytes = &buffer[..n];
                    if gate.intercept(bytes) {
                        continue;
                    }
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

fn spawn_local_resize_monitor(
    running: Arc<AtomicBool>,
    process: Arc<PtyProcess>,
    emulator: Arc<Mutex<Box<dyn TerminalEmulator + Send>>>,
    grid: Arc<TerminalGrid>,
    transports: Arc<Mutex<Vec<Arc<SharedTransport>>>>,
    local_server_transport: Arc<Mutex<Option<Arc<dyn Transport>>>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut last_size: Option<(u16, u16)> = None;
        while running.load(Ordering::Relaxed) {
            let (cols_raw, rows_raw) = detect_terminal_size();
            let cols = cols_raw.min(MAX_PTY_COLS);
            let rows = rows_raw.min(MAX_PTY_ROWS);
            let current = (cols, rows);

            if Some(current) != last_size {
                let mut applied = true;
                if let Err(err) = process.resize(cols, rows) {
                    warn!(
                        target = "host::local_resize",
                        cols,
                        rows,
                        error = %err,
                        "failed to apply PTY resize"
                    );
                    applied = false;
                }

                if applied {
                    if let Ok(mut emulator) = emulator.lock() {
                        emulator.resize(rows as usize, cols as usize);
                    }
                    grid.set_viewport_size(rows as usize, cols as usize);

                    let history_rows = grid.rows() as u32;
                    let base_row = grid.row_offset();

                    let transports_snapshot: Vec<Arc<SharedTransport>> = {
                        let guard = transports.lock().unwrap();
                        guard.iter().cloned().collect()
                    };

                    for shared in transports_snapshot {
                        let transport_id = shared.id().0;
                        let transport_kind = shared.kind();
                        let transport: Arc<dyn Transport> = shared;
                        if let Err(err) = send_host_frame(
                            &transport,
                            HostFrame::Grid {
                                cols: cols as u32,
                                history_rows,
                                base_row,
                                viewport_rows: Some(rows as u32),
                            },
                        ) {
                            warn!(
                                target = "host::local_resize",
                                transport_id,
                                transport = ?transport_kind,
                                error = %err,
                                "failed to broadcast viewport update"
                            );
                        }
                    }

                    if let Some(server) = local_server_transport.lock().unwrap().clone() {
                        let transport_id = server.id().0;
                        let transport_kind = server.kind();
                        if let Err(err) = send_host_frame(
                            &server,
                            HostFrame::Grid {
                                cols: cols as u32,
                                history_rows,
                                base_row,
                                viewport_rows: Some(rows as u32),
                            },
                        ) {
                            warn!(
                                target = "host::local_resize",
                                transport_id,
                                transport = ?transport_kind,
                                error = %err,
                                "failed to broadcast viewport update"
                            );
                        }
                    }

                    trace!(
                        target = "host::local_resize",
                        cols, rows, history_rows, base_row, "applied local terminal resize"
                    );

                    last_size = Some(current);
                }
            }

            std::thread::sleep(Duration::from_millis(200));
        }

        trace!(target = "host::local_resize", "resize monitor exiting");
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
    cursor: Option<CursorFrame>,
}

impl TransmitterCache {
    fn new() -> Self {
        Self::default()
    }

    fn reset(&mut self, cols: usize) {
        self.cols = cols;
        self.rows.clear();
        self.styles.clear();
        self.cursor = None;
    }

    fn apply_updates(&mut self, updates: &[CacheUpdate], dedupe: bool) -> PreparedUpdateBatch {
        let mut out = Vec::with_capacity(updates.len());
        let mut next_cursor: Option<CursorFrame> = None;
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
                    trace!(
                        target = "sync::transmitter",
                        start = trim.start,
                        count = trim.count,
                        seq = trim.seq(),
                        marker = "tail_base_row_v3"
                    );
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
                CacheUpdate::Cursor(cursor_state) => {
                    let candidate = CursorFrame {
                        row: usize_to_u32(cursor_state.row),
                        col: usize_to_u32(cursor_state.col),
                        seq: cursor_state.seq,
                        visible: cursor_state.visible,
                        blink: cursor_state.blink,
                    };
                    match next_cursor {
                        Some(ref existing) if existing.seq >= candidate.seq => {}
                        _ => next_cursor = Some(candidate),
                    }
                }
            }
        }
        let cursor = next_cursor.and_then(|candidate| {
            let emit = match self.cursor.as_ref() {
                Some(prev) => {
                    candidate.seq > prev.seq
                        || candidate.row != prev.row
                        || candidate.col != prev.col
                        || candidate.visible != prev.visible
                        || candidate.blink != prev.blink
                }
                None => true,
            };
            if emit {
                self.cursor = Some(candidate.clone());
                Some(candidate)
            } else {
                None
            }
        });

        PreparedUpdateBatch {
            updates: out,
            cursor,
        }
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

#[derive(Default)]
struct PreparedUpdateBatch {
    updates: Vec<WireUpdate>,
    cursor: Option<CursorFrame>,
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

enum ForwarderCommand {
    AddTransport {
        transport: Arc<dyn Transport>,
        supervisor: Option<Arc<TransportSupervisor>>,
    },
    RemoveTransport {
        id: TransportId,
    },
    ViewportRefresh,
}

fn spawn_webrtc_acceptor(
    session_id: String,
    session_handle: SessionHandle,
    join_code: Option<String>,
    writer: PtyWriter,
    process_handle: Arc<PtyProcess>,
    emulator_handle: Arc<Mutex<Box<dyn TerminalEmulator + Send>>>,
    grid: Arc<TerminalGrid>,
    backfill_tx: UnboundedSender<BackfillCommand>,
    input_handles: Arc<Mutex<Vec<thread::JoinHandle<()>>>>,
    forwarder_tx: UnboundedSender<ForwarderCommand>,
    transports: Arc<Mutex<Vec<Arc<SharedTransport>>>>,
    authorizer: Arc<JoinAuthorizer>,
    mcp_handle: Option<McpServerHandle>,
    mcp_bridges: Arc<Mutex<Vec<JoinHandle<()>>>>,
    ready_tx: Option<oneshot::Sender<()>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ready_tx = ready_tx;
        loop {
            let passphrase = join_code.as_deref();
            match negotiate_transport(&session_handle, passphrase, None, false).await {
                Ok(NegotiatedTransport::Single(NegotiatedSingle {
                    transport,
                    webrtc_channels,
                })) => {
                    let selected_kind = transport.kind();
                    info!(session_id = %session_id, transport = ?selected_kind, "transport negotiated");
                    let metadata = JoinAuthorizationMetadata::from_parts(
                        selected_kind,
                        None,
                        None,
                        Some("primary transport".to_string()),
                        HashMap::new(),
                    );
                    let hint_pending = authorizer.should_emit_pending_hint();
                    let auto_grant = authorizer.should_emit_auto_granted();
                    if hint_pending {
                        let _ = transport.send_text("beach:status:approval_pending");
                    }
                    if !authorizer.authorize(metadata.clone()).await {
                        if hint_pending {
                            let _ = transport.send_text("beach:status:approval_denied");
                        }
                        info!(
                            session_id = %session_id,
                            transport = ?selected_kind,
                            "client join denied"
                        );
                        continue;
                    }
                    if hint_pending || auto_grant {
                        let _ = transport.send_text("beach:status:approval_granted");
                    }

                    let shared_transport = Arc::new(SharedTransport::new(transport.clone()));
                    {
                        let mut guard = transports.lock().unwrap();
                        guard.push(shared_transport.clone());
                    }
                    let supervisor = Arc::new(TransportSupervisor::new(
                        shared_transport.clone(),
                        session_handle.clone(),
                        join_code.clone(),
                    ));
                    let primary_transport: Arc<dyn Transport> = shared_transport.clone();

                    if let (Some(handle), Some(channels)) =
                        (mcp_handle.clone(), webrtc_channels.clone())
                    {
                        let bridges = Arc::clone(&mcp_bridges);
                        let session_for_bridge = session_id.clone();
                        let parent_transport_id = primary_transport.id();
                        let parent_peer_id = primary_transport.peer();
                        let bridge_task = tokio::spawn(async move {
                            match timeout(MCP_CHANNEL_TIMEOUT, channels.wait_for(MCP_CHANNEL_LABEL))
                                .await
                            {
                                Ok(Ok(mcp_transport)) => {
                                    debug!(
                                        target = "mcp::bridge",
                                        session_id = %session_for_bridge,
                                        parent_transport_id = parent_transport_id.0,
                                        parent_peer_id = parent_peer_id.0,
                                        mcp_transport_id = mcp_transport.id().0,
                                        "attaching mcp bridge"
                                    );
                                    let bridge_handle = spawn_webrtc_bridge(
                                        handle,
                                        mcp_transport,
                                        MCP_CHANNEL_LABEL,
                                    );
                                    let _ = bridge_handle.await;
                                }
                                Ok(Err(err)) => {
                                    warn!(
                                        target = "mcp::bridge",
                                        session_id = %session_for_bridge,
                                        parent_transport_id = parent_transport_id.0,
                                        parent_peer_id = parent_peer_id.0,
                                        error = %err,
                                        "failed waiting for mcp channel"
                                    );
                                }
                                Err(_) => {
                                    debug!(
                                        target = "mcp::bridge",
                                        session_id = %session_for_bridge,
                                        parent_transport_id = parent_transport_id.0,
                                        parent_peer_id = parent_peer_id.0,
                                        timeout_secs = MCP_CHANNEL_TIMEOUT.as_secs(),
                                        "timed out waiting for mcp channel"
                                    );
                                }
                            }
                        });
                        bridges.lock().unwrap().push(bridge_task);
                    }

                    HeartbeatPublisher::new(primary_transport.clone(), Some(supervisor.clone()))
                        .spawn(Duration::from_secs(10), None);
                    let listener = spawn_input_listener(
                        primary_transport.clone(),
                        writer.clone(),
                        process_handle.clone(),
                        emulator_handle.clone(),
                        grid.clone(),
                        backfill_tx.clone(),
                        Some(forwarder_tx.clone()),
                        authorizer.gate(),
                    );
                    input_handles.lock().unwrap().push(listener);
                    if forwarder_tx
                        .send(ForwarderCommand::AddTransport {
                            transport: primary_transport.clone(),
                            supervisor: Some(supervisor.clone()),
                        })
                        .is_err()
                    {
                        warn!(
                            session_id = %session_id,
                            "update forwarder closed; stopping transport acceptor"
                        );
                        break;
                    }
                    if let Some(tx) = ready_tx.take() {
                        let _ = tx.send(());
                    }
                }
                Ok(NegotiatedTransport::WebRtcOfferer {
                    supervisor,
                    connection,
                    peer_id,
                    handshake_id,
                    metadata: extra_metadata,
                }) => {
                    let transport = connection.transport();
                    let channels = connection.channels();
                    info!(
                        session_id = %session_id,
                        transport = "webrtc-multi",
                        "transport negotiated with offerer supervisor"
                    );
                    let metadata = JoinAuthorizationMetadata::from_parts(
                        transport.kind(),
                        Some(peer_id.clone()),
                        Some(handshake_id.clone()),
                        Some("offerer supervisor".to_string()),
                        extra_metadata,
                    );
                    let hint_pending = authorizer.should_emit_pending_hint();
                    let auto_grant = authorizer.should_emit_auto_granted();
                    if hint_pending {
                        let _ = transport.send_text("beach:status:approval_pending");
                    }
                    if !authorizer.authorize(metadata.clone()).await {
                        if hint_pending {
                            let _ = transport.send_text("beach:status:approval_denied");
                        }
                        info!(
                            session_id = %session_id,
                            "offerer transport denied by host"
                        );
                        continue;
                    }
                    if hint_pending || auto_grant {
                        let _ = transport.send_text("beach:status:approval_granted");
                    }

                    let shared_transport = Arc::new(SharedTransport::new(transport.clone()));
                    {
                        let mut guard = transports.lock().unwrap();
                        guard.push(shared_transport.clone());
                    }
                    let primary_transport: Arc<dyn Transport> = shared_transport.clone();

                    if let Some(handle) = mcp_handle.clone() {
                        let bridges = Arc::clone(&mcp_bridges);
                        let parent_transport_id = primary_transport.id();
                        let parent_peer_id = primary_transport.peer();
                        let peer_for_bridge = peer_id.clone();
                        let handshake_for_bridge = handshake_id.clone();
                        let bridge_task = tokio::spawn(async move {
                            match timeout(MCP_CHANNEL_TIMEOUT, channels.wait_for(MCP_CHANNEL_LABEL))
                                .await
                            {
                                Ok(Ok(mcp_transport)) => {
                                    debug!(
                                        target = "mcp::bridge",
                                        peer_id = %peer_for_bridge,
                                        handshake_id = %handshake_for_bridge,
                                        parent_transport_id = parent_transport_id.0,
                                        parent_peer_id = parent_peer_id.0,
                                        mcp_transport_id = mcp_transport.id().0,
                                        "attaching mcp bridge for viewer"
                                    );
                                    let bridge_handle = spawn_webrtc_bridge(
                                        handle,
                                        mcp_transport,
                                        MCP_CHANNEL_LABEL,
                                    );
                                    let _ = bridge_handle.await;
                                }
                                Ok(Err(err)) => {
                                    warn!(
                                        target = "mcp::bridge",
                                        peer_id = %peer_for_bridge,
                                        handshake_id = %handshake_for_bridge,
                                        parent_transport_id = parent_transport_id.0,
                                        parent_peer_id = parent_peer_id.0,
                                        error = %err,
                                        "failed waiting for viewer mcp channel"
                                    );
                                }
                                Err(_) => {
                                    debug!(
                                        target = "mcp::bridge",
                                        peer_id = %peer_for_bridge,
                                        handshake_id = %handshake_for_bridge,
                                        parent_transport_id = parent_transport_id.0,
                                        parent_peer_id = parent_peer_id.0,
                                        timeout_secs = MCP_CHANNEL_TIMEOUT.as_secs(),
                                        "viewer did not open mcp channel before timeout"
                                    );
                                }
                            }
                        });
                        bridges.lock().unwrap().push(bridge_task);
                    }

                    HeartbeatPublisher::new(primary_transport.clone(), None)
                        .spawn(Duration::from_secs(10), None);
                    let listener = spawn_input_listener(
                        primary_transport.clone(),
                        writer.clone(),
                        process_handle.clone(),
                        emulator_handle.clone(),
                        grid.clone(),
                        backfill_tx.clone(),
                        Some(forwarder_tx.clone()),
                        authorizer.gate(),
                    );
                    input_handles.lock().unwrap().push(listener);
                    if forwarder_tx
                        .send(ForwarderCommand::AddTransport {
                            transport: primary_transport.clone(),
                            supervisor: None,
                        })
                        .is_err()
                    {
                        warn!(
                            session_id = %session_id,
                            "update forwarder closed; stopping transport acceptor"
                        );
                        break;
                    }
                    if let Some(tx) = ready_tx.take() {
                        let _ = tx.send(());
                    }

                    spawn_viewer_accept_loop(
                        supervisor,
                        forwarder_tx.clone(),
                        writer.clone(),
                        process_handle.clone(),
                        emulator_handle.clone(),
                        grid.clone(),
                        backfill_tx.clone(),
                        Arc::clone(&input_handles),
                        Arc::clone(&transports),
                        Arc::clone(&authorizer),
                        mcp_handle.clone(),
                        Arc::clone(&mcp_bridges),
                    );
                    break;
                }
                Err(err) => {
                    warn!(
                        session_id = %session_id,
                        error = %err,
                        "transport negotiation failed; retrying"
                    );
                    sleep(Duration::from_secs(1)).await;
                }
            }
        }
    })
}

fn spawn_viewer_accept_loop(
    supervisor: Arc<OffererSupervisor>,
    forwarder_tx: UnboundedSender<ForwarderCommand>,
    writer: PtyWriter,
    process_handle: Arc<PtyProcess>,
    emulator_handle: Arc<Mutex<Box<dyn TerminalEmulator + Send>>>,
    grid: Arc<TerminalGrid>,
    backfill_tx: UnboundedSender<BackfillCommand>,
    input_handles: Arc<Mutex<Vec<thread::JoinHandle<()>>>>,
    transports: Arc<Mutex<Vec<Arc<SharedTransport>>>>,
    authorizer: Arc<JoinAuthorizer>,
    mcp_handle: Option<McpServerHandle>,
    mcp_bridges: Arc<Mutex<Vec<JoinHandle<()>>>>,
) {
    tokio::spawn(async move {
        loop {
            match supervisor.next().await {
                Ok(accepted) => {
                    let OffererAcceptedTransport {
                        peer_id,
                        handshake_id,
                        metadata: extra_metadata,
                        connection,
                    } = accepted;
                    let transport = connection.transport();
                    let channels = connection.channels();
                    let transport_arc: Arc<dyn Transport> = transport.clone();
                    let auth_metadata = JoinAuthorizationMetadata::from_parts(
                        transport_arc.kind(),
                        Some(peer_id.clone()),
                        Some(handshake_id.clone()),
                        Some("viewer".to_string()),
                        extra_metadata,
                    );
                    let hint_pending = authorizer.should_emit_pending_hint();
                    let auto_grant = authorizer.should_emit_auto_granted();
                    if hint_pending {
                        let _ = transport_arc.send_text("beach:status:approval_pending");
                    }
                    if !authorizer.authorize(auth_metadata.clone()).await {
                        if hint_pending {
                            let _ = transport_arc.send_text("beach:status:approval_denied");
                        }
                        info!(
                            target = "webrtc",
                            peer_id = %peer_id,
                            handshake_id = %handshake_id,
                            "viewer transport denied by host"
                        );
                        continue;
                    }
                    if hint_pending || auto_grant {
                        let _ = transport_arc.send_text("beach:status:approval_granted");
                    }

                    let shared_transport = Arc::new(SharedTransport::new(transport_arc.clone()));
                    {
                        let mut guard = transports.lock().unwrap();
                        guard.push(shared_transport.clone());
                    }
                    let shared_arc: Arc<dyn Transport> = shared_transport.clone();

                    if let Some(handle) = mcp_handle.clone() {
                        let bridges = Arc::clone(&mcp_bridges);
                        let parent_transport_id = shared_arc.id();
                        let parent_peer_id = shared_arc.peer();
                        let peer_for_bridge = peer_id.clone();
                        let handshake_for_bridge = handshake_id.clone();
                        let channels_clone = channels.clone();
                        let bridge_task = tokio::spawn(async move {
                            match timeout(
                                MCP_CHANNEL_TIMEOUT,
                                channels_clone.wait_for(MCP_CHANNEL_LABEL),
                            )
                            .await
                            {
                                Ok(Ok(mcp_transport)) => {
                                    debug!(
                                        target = "mcp::bridge",
                                        peer_id = %peer_for_bridge,
                                        handshake_id = %handshake_for_bridge,
                                        parent_transport_id = parent_transport_id.0,
                                        parent_peer_id = parent_peer_id.0,
                                        mcp_transport_id = mcp_transport.id().0,
                                        "viewer attached mcp bridge"
                                    );
                                    let bridge_handle = spawn_webrtc_bridge(
                                        handle,
                                        mcp_transport,
                                        MCP_CHANNEL_LABEL,
                                    );
                                    let _ = bridge_handle.await;
                                }
                                Ok(Err(err)) => {
                                    warn!(
                                        target = "mcp::bridge",
                                        peer_id = %peer_for_bridge,
                                        handshake_id = %handshake_for_bridge,
                                        parent_transport_id = parent_transport_id.0,
                                        parent_peer_id = parent_peer_id.0,
                                        error = %err,
                                        "viewer mcp channel failed"
                                    );
                                }
                                Err(_) => {
                                    debug!(
                                        target = "mcp::bridge",
                                        peer_id = %peer_for_bridge,
                                        handshake_id = %handshake_for_bridge,
                                        parent_transport_id = parent_transport_id.0,
                                        parent_peer_id = parent_peer_id.0,
                                        timeout_secs = MCP_CHANNEL_TIMEOUT.as_secs(),
                                        "viewer did not create mcp channel"
                                    );
                                }
                            }
                        });
                        bridges.lock().unwrap().push(bridge_task);
                    }
                    HeartbeatPublisher::new(shared_arc.clone(), None)
                        .spawn(Duration::from_secs(10), None);

                    let listener = spawn_input_listener(
                        shared_arc.clone(),
                        writer.clone(),
                        process_handle.clone(),
                        emulator_handle.clone(),
                        grid.clone(),
                        backfill_tx.clone(),
                        Some(forwarder_tx.clone()),
                        authorizer.gate(),
                    );
                    input_handles.lock().unwrap().push(listener);

                    if forwarder_tx
                        .send(ForwarderCommand::AddTransport {
                            transport: shared_arc,
                            supervisor: None,
                        })
                        .is_err()
                    {
                        break;
                    }

                    info!(
                        target = "webrtc",
                        peer_id = %peer_id,
                        handshake_id = %handshake_id,
                        "viewer transport registered"
                    );
                }
                Err(_) => break,
            }
        }
    });
}

fn spawn_update_forwarder(
    transports: Vec<(Arc<dyn Transport>, Option<Arc<TransportSupervisor>>)>,
    mut updates: UnboundedReceiver<CacheUpdate>,
    timeline: Arc<TimelineDeltaStream>,
    terminal_sync: Arc<TerminalSync>,
    sync_config: SyncConfig,
    mut backfill_rx: UnboundedReceiver<BackfillCommand>,
    mut command_rx: UnboundedReceiver<ForwarderCommand>,
    forwarder_tx: Option<UnboundedSender<ForwarderCommand>>,
    shared_registry: Arc<Mutex<Vec<Arc<SharedTransport>>>>,
    cursor_sync: bool,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        struct Sink {
            transport: Arc<dyn Transport>,
            supervisor: Option<Arc<TransportSupervisor>>,
            synchronizer: ServerSynchronizer<TerminalSync, CacheUpdate>,
            last_seq: Seq,
            active: bool,
            handshake_complete: bool,
            last_handshake: Instant,
            handshake_attempts: u32,
            cache: TransmitterCache,
            backfill_queue: VecDeque<BackfillJob>,
            last_backfill_sent: Option<Instant>,
        }

        const HANDSHAKE_REFRESH: Duration = Duration::from_millis(200);

        let forwarder_tx = forwarder_tx;

        fn is_data_channel_not_open(err: &TransportError) -> bool {
            matches!(err, TransportError::Setup(message) if message.contains("DataChannel is not opened"))
        }

        fn drop_transport(
            sinks: &mut Vec<Sink>,
            shared_registry: &Arc<Mutex<Vec<Arc<SharedTransport>>>>,
            id: TransportId,
        ) {
            let before = sinks.len();
            sinks.retain(|sink| sink.transport.id() != id);
            if sinks.len() < before {
                info!(
                    target = "sync::forwarder",
                    transport_id = id.0,
                    removed = before - sinks.len(),
                    "removed transport sink"
                );
            } else {
                debug!(
                    target = "sync::forwarder",
                    transport_id = id.0,
                    "remove command ignored: transport not found"
                );
            }
            let mut registry = shared_registry.lock().unwrap();
            let registry_before = registry.len();
            registry.retain(|shared| shared.id() != id);
            if registry.len() < registry_before {
                trace!(
                    target = "sync::forwarder",
                    transport_id = id.0,
                    removed = registry_before - registry.len(),
                    "pruned shared transport registry"
                );
            }
        }

        fn request_transport_removal(
            id: TransportId,
            forwarder_tx: &Option<UnboundedSender<ForwarderCommand>>,
            sinks: &mut Vec<Sink>,
            shared_registry: &Arc<Mutex<Vec<Arc<SharedTransport>>>>,
        ) {
            let mut dispatched = false;
            if let Some(tx) = forwarder_tx {
                match tx.send(ForwarderCommand::RemoveTransport { id }) {
                    Ok(()) => {
                        dispatched = true;
                        trace!(
                            target = "sync::forwarder",
                            transport_id = id.0,
                            "enqueued transport removal command"
                        );
                    }
                    Err(_) => {
                        trace!(
                            target = "sync::forwarder",
                            transport_id = id.0,
                            "failed to enqueue transport removal; removing locally"
                        );
                    }
                }
            }
            if !dispatched {
                drop_transport(sinks, shared_registry, id);
            }
        }

        let subscription = SubscriptionId(1);
        let grid = terminal_sync.grid().clone();
        let mut next_backfill_index: usize = 0;
        let mut sinks: Vec<Sink> = transports
            .into_iter()
            .map(|(transport, supervisor)| Sink {
                synchronizer: ServerSynchronizer::new(terminal_sync.clone(), sync_config.clone()),
                transport,
                supervisor,
                last_seq: 0,
                active: true,
                handshake_complete: false,
                last_handshake: Instant::now(),
                handshake_attempts: 0,
                cache: TransmitterCache::new(),
                backfill_queue: VecDeque::new(),
                last_backfill_sent: None,
            })
            .collect();

        let mut stale_transports: Vec<TransportId> = Vec::new();

        for sink in sinks.iter_mut() {
            match initialize_transport_snapshot(
                &sink.transport,
                subscription,
                &terminal_sync,
                &sync_config,
                &mut sink.cache,
                cursor_sync,
            ) {
                Ok((sync, seq)) => {
                    sink.synchronizer = sync;
                    sink.last_seq = seq;
                    sink.handshake_complete = true;
                }
                Err(err) => {
                    sink.handshake_complete = false;
                    let transport_id = sink.transport.id();
                    if is_data_channel_not_open(&err) {
                        sink.active = false;
                        sink.backfill_queue.clear();
                        stale_transports.push(transport_id);
                        warn!(
                            target = "sync::handshake",
                            transport_id = transport_id.0,
                            transport = ?sink.transport.kind(),
                            error = %err,
                            "initial handshake failed: data channel not open"
                        );
                    } else {
                        warn!(
                            target = "sync::handshake",
                            transport_id = transport_id.0,
                            transport = ?sink.transport.kind(),
                            error = %err,
                            "initial handshake failed"
                        );
                    }
                    if let Some(supervisor) = &sink.supervisor {
                        supervisor.schedule_reconnect();
                    }
                }
            }
            sink.last_handshake = Instant::now();
        }

        if !stale_transports.is_empty() {
            for id in stale_transports.drain(..) {
                request_transport_removal(id, &forwarder_tx, &mut sinks, &shared_registry);
            }
        }

        fn attempt_handshake(
            sink: &mut Sink,
            subscription: SubscriptionId,
            terminal_sync: &Arc<TerminalSync>,
            sync_config: &SyncConfig,
            stale_transports: &mut Vec<TransportId>,
            cursor_sync: bool,
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
            match initialize_transport_snapshot(
                &sink.transport,
                subscription,
                terminal_sync,
                sync_config,
                &mut sink.cache,
                cursor_sync,
            ) {
                Ok((sync, seq)) => {
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
                }
                Err(err) => {
                    sink.handshake_complete = false;
                    let transport_id = sink.transport.id();
                    if is_data_channel_not_open(&err) {
                        sink.active = false;
                        sink.backfill_queue.clear();
                        stale_transports.push(transport_id);
                        warn!(
                            target = "sync::handshake",
                            transport_id = transport_id.0,
                            transport = ?sink.transport.kind(),
                            error = %err,
                            "handshake attempt failed: data channel not open"
                        );
                    } else {
                        debug!(
                            target = "sync::handshake",
                            transport_id = transport_id.0,
                            transport = ?sink.transport.kind(),
                            error = %err,
                            "handshake attempt did not complete"
                        );
                    }
                    if let Some(supervisor) = &sink.supervisor {
                        supervisor.schedule_reconnect();
                    }
                }
            }
        }

        let mut handshake_timer = interval(Duration::from_millis(200));

        loop {
            tokio::select! {
                _ = handshake_timer.tick() => {
                    for sink in sinks.iter_mut().filter(|s| s.active && !s.handshake_complete) {
                        if sink.last_handshake.elapsed() < HANDSHAKE_REFRESH {
                            continue;
                        }
                        attempt_handshake(
                            sink,
                            subscription,
                            &terminal_sync,
                            &sync_config,
                            &mut stale_transports,
                            cursor_sync,
                        );
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
                                    let converted_batch = sink.cache.apply_updates(&batch.updates, true);
                                    let _guard = PerfGuard::new("sync_send_delta");
                                    match send_delta_frames_chunked(
                                        &sink.transport,
                                        batch.subscription_id,
                                        batch.watermark.0,
                                        batch.has_more,
                                        converted_batch,
                                    ) {
                                        Ok(()) => {
                                            sink.last_seq = batch.watermark.0;
                                            sink.last_handshake = Instant::now();
                                            batches_sent = batches_sent.saturating_add(1);
                                        }
                                        Err(err) => {
                                            let transport_id = sink.transport.id();
                                            sink.handshake_complete = false;
                                            if is_data_channel_not_open(&err) {
                                                sink.active = false;
                                                sink.backfill_queue.clear();
                                                stale_transports.push(transport_id);
                                                warn!(
                                                    target = "sync::handshake",
                                                    transport_id = transport_id.0,
                                                    transport = ?sink.transport.kind(),
                                                    error = %err,
                                                    "delta send failed: data channel not open"
                                                );
                                            } else {
                                                warn!(
                                                    target = "sync::handshake",
                                                    transport_id = transport_id.0,
                                                    transport = ?sink.transport.kind(),
                                                    error = %err,
                                                    "delta send failed, marking handshake incomplete"
                                                );
                                            }
                                            if let Some(supervisor) = &sink.supervisor {
                                                supervisor.schedule_reconnect();
                                            }
                                            break;
                                        }
                                    }
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
                maybe_forwarder = command_rx.recv() => {
                    if let Some(command) = maybe_forwarder {
                        match command {
                            ForwarderCommand::AddTransport { transport, supervisor } => {
                                let mut sink = Sink {
                                    synchronizer: ServerSynchronizer::new(
                                        terminal_sync.clone(),
                                        sync_config.clone(),
                                    ),
                                    transport: transport.clone(),
                                    supervisor,
                                    last_seq: 0,
                                    active: true,
                                    handshake_complete: false,
                                    last_handshake: Instant::now(),
                                    handshake_attempts: 0,
                                    cache: TransmitterCache::new(),
                                    backfill_queue: VecDeque::new(),
                                    last_backfill_sent: None,
                                };

                                match initialize_transport_snapshot(
                                    &sink.transport,
                                    subscription,
                                    &terminal_sync,
                                    &sync_config,
                                    &mut sink.cache,
                                    cursor_sync,
                                ) {
                                    Ok((sync, seq)) => {
                                        sink.synchronizer = sync;
                                        sink.last_seq = seq;
                                        sink.handshake_complete = true;
                                    }
                                    Err(err) => {
                                        sink.handshake_complete = false;
                                        let transport_id = sink.transport.id();
                                        if is_data_channel_not_open(&err) {
                                            sink.active = false;
                                            sink.backfill_queue.clear();
                                            stale_transports.push(transport_id);
                                            warn!(
                                                target = "sync::handshake",
                                                transport_id = transport_id.0,
                                                transport = ?sink.transport.kind(),
                                                error = %err,
                                                "handshake failed for new transport: data channel not open"
                                            );
                                        } else {
                                            warn!(
                                                target = "sync::handshake",
                                                transport_id = transport_id.0,
                                                transport = ?sink.transport.kind(),
                                                error = %err,
                                                "handshake failed for new transport"
                                            );
                                        }
                                        if let Some(supervisor) = &sink.supervisor {
                                            supervisor.schedule_reconnect();
                                        }
                                    }
                                }
                                sink.last_handshake = Instant::now();
                                sinks.push(sink);
                            }
                            ForwarderCommand::RemoveTransport { id } => {
                                drop_transport(&mut sinks, &shared_registry, id);
                            }
                            ForwarderCommand::ViewportRefresh => {
                                let (_, cols) = grid.viewport_size();
                                for sink in sinks.iter_mut() {
                                    if !sink.active {
                                        continue;
                                    }
                                    sink.synchronizer.reset();
                                    sink.cache.reset(cols);
                                    sink.handshake_complete = false;
                                    sink.handshake_attempts = 0;
                                    sink.last_handshake = Instant::now() - HANDSHAKE_REFRESH;
                                }
                            }
                        }
                    }
                }
                maybe_command = backfill_rx.recv() => {
                    match maybe_command {
                        Some(command) => {
                            let end_row = command.start_row.saturating_add(command.count as u64);
                            if end_row <= command.start_row {
                                continue;
                            }
                            if let Some(sink) = sinks
                                .iter_mut()
                                .find(|s| s.transport.id() == command.transport_id)
                            {
                                sink.backfill_queue.push_back(BackfillJob {
                                    subscription: command.subscription,
                                    request_id: command.request_id,
                                    next_row: command.start_row,
                                    end_row,
                                });
                                trace!(
                                    target = "sync::backfill",
                                    transport_id = sink.transport.id().0,
                                    request_id = command.request_id,
                                    start_row = command.start_row,
                                    count = command.count,
                                    queued = sink.backfill_queue.len(),
                                    "enqueued backfill request"
                                );
                            } else {
                                debug!(
                                    target = "sync::backfill",
                                    transport = command.transport_id.0,
                                    "backfill request dropped: transport not found"
                                );
                            }
                        }
                        None => {}
                    }
                }
            }

            let sink_count = sinks.len();
            if sink_count > 0 {
                if next_backfill_index >= sink_count {
                    next_backfill_index = 0;
                }
                for _ in 0..sink_count {
                    if sinks.is_empty() {
                        break;
                    }
                    if next_backfill_index >= sinks.len() {
                        next_backfill_index = 0;
                    }
                    let idx = next_backfill_index;
                    next_backfill_index = (next_backfill_index + 1) % sinks.len().max(1);
                    let sink = &mut sinks[idx];
                    if !sink.active || !sink.handshake_complete {
                        continue;
                    }
                    if sink.backfill_queue.is_empty() {
                        continue;
                    }
                    if let Some(last) = sink.last_backfill_sent {
                        if last.elapsed() < SERVER_BACKFILL_THROTTLE {
                            continue;
                        }
                    }
                    let mut job = match sink.backfill_queue.pop_front() {
                        Some(job) => job,
                        None => continue,
                    };
                    if job.next_row >= job.end_row {
                        continue;
                    }
                    let chunk_start = job.next_row;
                    let remaining = job.end_row.saturating_sub(chunk_start);
                    let chunk_rows = remaining
                        .min(MAX_BACKFILL_ROWS_PER_REQUEST as u64)
                        .min(SERVER_BACKFILL_CHUNK_ROWS as u64)
                        as u32;
                    let chunk = collect_backfill_chunk(&grid, chunk_start, chunk_rows);
                    let chunk_advance = chunk.attempted as u64;
                    let next_row = chunk_start.saturating_add(chunk_advance);
                    let more_pending = next_row < job.end_row;
                    let request_id = job.request_id;
                    let converted_batch = sink.cache.apply_updates(&chunk.updates, false);
                    match send_host_frame(
                        &sink.transport,
                        HostFrame::HistoryBackfill {
                            subscription: job.subscription,
                            request_id: job.request_id,
                            start_row: chunk_start,
                            count: chunk.attempted,
                            updates: converted_batch.updates,
                            more: more_pending,
                            cursor: converted_batch.cursor,
                        },
                    ) {
                        Ok(()) => {
                            sink.last_backfill_sent = Some(Instant::now());
                            job.next_row = next_row;
                            if more_pending {
                                sink.backfill_queue.push_back(job);
                            }
                            trace!(
                                target = "sync::backfill",
                                transport_id = sink.transport.id().0,
                                request_id,
                                start_row = chunk_start,
                                count = chunk.attempted,
                                delivered = chunk.delivered,
                                more = more_pending,
                                "sent backfill chunk"
                            );
                        }
                        Err(err) => {
                            let transport_id = sink.transport.id();
                            sink.handshake_complete = false;
                            if is_data_channel_not_open(&err) {
                                sink.active = false;
                                sink.backfill_queue.clear();
                                stale_transports.push(transport_id);
                                warn!(
                                    target = "sync::backfill",
                                    transport_id = transport_id.0,
                                    transport = ?sink.transport.kind(),
                                    error = %err,
                                    "backfill send failed: data channel not open"
                                );
                            } else {
                                sink.backfill_queue.push_front(job);
                                warn!(
                                    target = "sync::backfill",
                                    transport_id = transport_id.0,
                                    transport = ?sink.transport.kind(),
                                    error = %err,
                                    "backfill send failed; scheduling reconnect"
                                );
                            }
                            if let Some(supervisor) = &sink.supervisor {
                                supervisor.schedule_reconnect();
                            }
                        }
                    }
                    break;
                }
            }

            if !stale_transports.is_empty() {
                for id in stale_transports.drain(..) {
                    request_transport_removal(id, &forwarder_tx, &mut sinks, &shared_registry);
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
    cursor_sync: bool,
) -> Result<(ServerSynchronizer<TerminalSync, CacheUpdate>, Seq), TransportError> {
    let mut synchronizer = ServerSynchronizer::new(terminal_sync.clone(), sync_config.clone());
    let hello = synchronizer.hello(subscription);
    let features = if cursor_sync { FEATURE_CURSOR_SYNC } else { 0 };
    debug!(
        target = "sync::handshake",
        transport_id = transport.id().0,
        transport = ?transport.kind(),
        "sending server hello"
    );
    send_host_frame(
        transport,
        HostFrame::Hello {
            subscription: hello.subscription_id.0,
            max_seq: hello.max_seq.0,
            config: sync_config_to_wire(&hello.config),
            features,
        },
    )?;
    let (viewport_rows, cols) = terminal_sync.grid().viewport_size();
    let history_rows = terminal_sync.grid().rows();
    cache.reset(cols);
    debug!(
        target = "sync::handshake",
        transport_id = transport.id().0,
        transport = ?transport.kind(),
        viewport_rows,
        cols,
        history_rows,
        "sending grid descriptor"
    );
    send_host_frame(
        transport,
        HostFrame::Grid {
            cols: cols as u32,
            history_rows: history_rows as u32,
            base_row: terminal_sync.grid().row_offset(),
            viewport_rows: None,
        },
    )?;
    transmit_initial_snapshots(transport, &mut synchronizer, cache, subscription)?;
    debug!(
        target = "sync::handshake",
        transport_id = transport.id().0,
        transport = ?transport.kind(),
        watermark = hello.max_seq.0,
        "initial snapshots transmitted"
    );
    debug!(
        target = "sync::handshake",
        transport_id = transport.id().0,
        transport = ?transport.kind(),
        lanes = 3usize,
        watermark = hello.max_seq.0,
        "initial snapshots complete"
    );
    Ok((synchronizer, hello.max_seq.0))
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
        initial_snapshot_lines: config.initial_snapshot_lines as u32,
    }
}

fn transmit_initial_snapshots(
    transport: &Arc<dyn Transport>,
    synchronizer: &mut ServerSynchronizer<TerminalSync, CacheUpdate>,
    cache: &mut TransmitterCache,
    subscription: SubscriptionId,
) -> Result<(), TransportError> {
    let transport_id = transport.id().0;
    let transport_kind = transport.kind();
    for lane in [
        PriorityLane::Foreground,
        PriorityLane::Recent,
        PriorityLane::History,
    ] {
        let mut emitted_chunk = false;
        while let Some(chunk) = synchronizer.snapshot_chunk(subscription, lane) {
            emitted_chunk = true;
            debug!(
                target = "sync::handshake",
                transport_id,
                transport = ?transport_kind,
                lane = ?lane,
                updates = chunk.updates.len(),
                "sending snapshot chunk"
            );
            let converted_batch = cache.apply_updates(&chunk.updates, false);
            send_snapshot_frames_chunked(
                transport,
                chunk.subscription_id,
                lane,
                chunk.watermark.0,
                chunk.has_more,
                converted_batch,
            )?;
            if !chunk.has_more {
                debug!(
                    target = "sync::handshake",
                    transport_id,
                    transport = ?transport_kind,
                    lane = ?lane,
                    "lane snapshot complete"
                );
                send_host_frame(
                    transport,
                    HostFrame::SnapshotComplete {
                        subscription: subscription.0,
                        lane: map_lane(lane),
                    },
                )?;
            }
        }
        if !emitted_chunk {
            debug!(
                target = "sync::handshake",
                transport_id,
                transport = ?transport_kind,
                lane = ?lane,
                "lane snapshot empty; sending completion"
            );
            send_host_frame(
                transport,
                HostFrame::SnapshotComplete {
                    subscription: subscription.0,
                    lane: map_lane(lane),
                },
            )?;
        }
    }
    Ok(())
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
    use crate::cache::terminal::{self, PackedCell, Style, StyleId};
    use crate::model::terminal::diff::{
        CellWrite, HistoryTrim, RectFill, RowSnapshot, StyleDefinition,
    };
    use crate::protocol::terminal::bootstrap::BootstrapHandshake;
    use crate::protocol::{
        self, ClientFrame as WireClientFrame, HostFrame as WireHostFrame, Lane as WireLane,
        Update as WireUpdate,
    };
    use crate::session::TransportOffer;
    use crate::sync::terminal::NullTerminalDeltaStream;
    use crate::terminal::app::interpret_session_target;
    use crate::transport::{Payload, TransportKind, TransportPair};
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration as StdDuration, Instant};
    use tokio::io::BufReader;
    use tokio::io::{AsyncWriteExt, duplex};
    use tokio::time::{Instant as TokioInstant, sleep, timeout};
    use uuid::Uuid;

    #[test]
    fn bootstrap_handshake_serializes_expected_fields() {
        let offers = vec![
            TransportOffer::WebRtc {
                offer: json!({"type": "offer"}),
            },
            TransportOffer::WebSocket {
                url: "wss://example.invalid/ws".to_string(),
            },
        ];
        let command = vec!["/bin/zsh".to_string(), "-l".to_string()];
        let handshake = BootstrapHandshake::from_context(
            "session-123",
            "012345",
            "http://127.0.0.1:8080",
            &offers,
            TransportKind::WebRtc,
            &command,
            true,
            false,
        );
        assert_eq!(handshake.schema, BootstrapHandshake::SCHEMA_VERSION);
        assert_eq!(handshake.session_id, "session-123");
        assert_eq!(handshake.join_code, "012345");
        assert_eq!(handshake.session_server, "http://127.0.0.1:8080");
        assert_eq!(
            handshake.transports,
            vec!["webrtc".to_string(), "websocket".to_string()]
        );
        assert_eq!(handshake.preferred_transport.as_deref(), Some("webrtc"));
        assert!(handshake.wait_for_peer);
        let serialized = serde_json::to_string(&handshake).expect("serializes to json");
        assert!(serialized.contains("\"session_id\":\"session-123\""));
        assert!(serialized.contains("\"command\":[\"/bin/zsh\",\"-l\"]"));
    }

    #[tokio::test]
    async fn read_bootstrap_handshake_skips_noise_lines() {
        let payload = json!({
            "schema": BootstrapHandshake::SCHEMA_VERSION,
            "session_id": "abc123",
            "join_code": "654321",
            "session_server": "http://localhost:8080",
            "active_transport": "WebRTC",
            "transports": ["webrtc"],
            "host_binary": "beach",
            "host_version": "0.1.0",
            "timestamp": 0,
            "command": ["/bin/sh"],
            "wait_for_peer": true,
            "mcp_enabled": false
        })
        .to_string();
        let script = format!("Last login: today\n{}\n", payload);

        let (mut writer, reader) = duplex(256);
        let mut reader = BufReader::new(reader);
        tokio::spawn(async move {
            writer
                .write_all(script.as_bytes())
                .await
                .expect("write handshake");
            writer.shutdown().await.expect("close writer");
        });

        let mut captured = Vec::new();
        let handshake =
            bootstrap::read_bootstrap_handshake(&mut reader, &mut captured, Duration::from_secs(2))
                .await
                .expect("handshake decoded");

        assert_eq!(captured, vec!["Last login: today".to_string()]);
        assert_eq!(handshake.session_id, "abc123");
        assert_eq!(handshake.join_code, "654321");
    }

    #[test]
    fn shell_quote_handles_spaces_and_quotes() {
        assert_eq!(bootstrap::shell_quote("simple"), "'simple'");
        assert_eq!(bootstrap::shell_quote("with space"), "'with space'");
        assert_eq!(bootstrap::shell_quote("path'with"), "'path'\"'\"'with'");

        let cmd = vec!["/opt/beach nightly".to_string(), "--flag".to_string()];
        let rendered = bootstrap::render_remote_command(&cmd);
        assert!(rendered.starts_with("exec '"));
        assert!(rendered.contains("'/opt/beach nightly'"));
    }

    #[test]
    fn scp_destination_defaults_to_target_prefix() {
        let dest = bootstrap::scp_destination("user@example.com", "beach");
        assert_eq!(dest, "user@example.com:beach");
    }

    #[test]
    fn scp_destination_respects_explicit_remote() {
        let dest = bootstrap::scp_destination("user@example.com", "root@other:/opt/beach");
        assert_eq!(dest, "root@other:/opt/beach");
    }

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

    #[allow(dead_code)]
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

    #[allow(dead_code)]
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

    #[test_timeout::timeout]
    fn parse_plain_session_id() {
        let id = Uuid::new_v4().to_string();
        let (parsed, base) = interpret_session_target(&id).unwrap();
        assert_eq!(parsed, id);
        assert!(base.is_none());
    }

    #[test_timeout::timeout]
    fn transmitter_cache_dedupes_rows_and_styles() {
        fn pack_cell(ch: char) -> PackedCell {
            PackedCell::from_raw(((ch as u32 as u64) << 32) | StyleId::DEFAULT.0 as u64)
        }

        let mut cache = TransmitterCache::new();
        cache.reset(4);

        let row_update =
            CacheUpdate::Row(RowSnapshot::new(0, 1, vec![pack_cell('h'), pack_cell('i')]));
        let first_emit = cache.apply_updates(&[row_update.clone()], false);
        assert_eq!(first_emit.updates.len(), 1, "initial row should emit");

        let second_emit = cache.apply_updates(&[row_update.clone()], true);
        assert!(
            second_emit.updates.is_empty(),
            "duplicate row should dedupe"
        );

        let cell_update = CacheUpdate::Cell(CellWrite::new(0, 1, 2, pack_cell('o')));
        let cell_emit = cache.apply_updates(&[cell_update.clone()], true);
        assert_eq!(cell_emit.updates.len(), 1, "cell change should emit once");
        let repeat_cell = cache.apply_updates(&[cell_update], true);
        assert!(
            repeat_cell.updates.is_empty(),
            "repeated cell should dedupe"
        );

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
        assert_eq!(style_emit.updates.len(), 1);
        let style_repeat = cache.apply_updates(&[CacheUpdate::Style(style)], true);
        assert!(
            style_repeat.updates.is_empty(),
            "duplicate style should dedupe"
        );

        let trim = CacheUpdate::Trim(HistoryTrim::new(0, 1));
        let trim_emit = cache.apply_updates(&[trim.clone()], true);
        assert_eq!(trim_emit.updates.len(), 1, "trim should always emit");

        let rect = CacheUpdate::Rect(RectFill::new(0..1, 0..2, 4, pack_cell('x')));
        let rect_emit = cache.apply_updates(&[rect.clone()], true);
        assert_eq!(rect_emit.updates.len(), 1, "rect change should emit");
        let rect_repeat = cache.apply_updates(&[rect], true);
        assert!(
            rect_repeat.updates.is_empty(),
            "identical rect should dedupe"
        );
    }

    #[test_timeout::timeout]
    fn parse_url_with_join_suffix() {
        let id = Uuid::new_v4();
        let url = format!("https://example.com/api/sessions/{}/join", id);
        let (parsed, base) = interpret_session_target(&url).unwrap();
        assert_eq!(parsed, id.to_string());
        assert_eq!(base.unwrap(), "https://example.com/api/");
    }

    #[test_timeout::timeout]
    fn parse_url_without_sessions_segment() {
        let id = Uuid::new_v4();
        let url = format!("https://example.com/{id}");
        let (parsed, base) = interpret_session_target(&url).unwrap();
        assert_eq!(parsed, id.to_string());
        assert_eq!(base.unwrap(), "https://example.com/");
    }

    #[test_timeout::timeout]
    fn reject_non_uuid_target() {
        let err = interpret_session_target("not-a-session").unwrap_err();
        assert!(matches!(err, CliError::InvalidSessionTarget { .. }));
    }

    #[test_timeout::tokio_timeout_test]
    async fn webrtc_mock_session_flow() {
        timeout(StdDuration::from_secs(30), async {
            let events = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));

            let pair = transport_mod::webrtc::create_test_pair()
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
            let (_backfill_tx, backfill_rx) = tokio::sync::mpsc::unbounded_channel();
            let (_forwarder_tx, forwarder_rx) = tokio::sync::mpsc::unbounded_channel();
            let forwarder = spawn_update_forwarder(
                vec![(Arc::clone(&server), None)],
                update_rx,
                timeline.clone(),
                terminal_sync.clone(),
                sync_config.clone(),
                backfill_rx,
                forwarder_rx,
                None,
                Arc::new(Mutex::new(Vec::new())),
                false,
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
                    cols: grid_cols,
                    history_rows,
                    base_row,
                    viewport_rows,
                } => {
                    assert!(
                        viewport_rows.is_none(),
                        "handshake should not advertise viewport rows"
                    );
                    assert_eq!(grid_cols as usize, cols);
                    assert!(
                        history_rows as usize >= rows,
                        "history rows should cover viewport"
                    );
                    assert_eq!(base_row, 0, "handshake base row should be 0 for fresh grid");
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
                    WireHostFrame::HistoryBackfill { .. } => {}
                    WireHostFrame::Heartbeat { .. } => {}
                    WireHostFrame::Hello { .. }
                    | WireHostFrame::Grid { .. }
                    | WireHostFrame::InputAck { .. }
                    | WireHostFrame::Cursor { .. }
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
                    | WireHostFrame::HistoryBackfill { .. }
                    | WireHostFrame::InputAck { .. }
                    | WireHostFrame::Cursor { .. }
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
                    data: b"echo world\n".to_vec(),
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

        HeartbeatPublisher::new(publisher_transport, None)
            .spawn(StdDuration::from_millis(10), Some(3));

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
    async fn handshake_refresh_stops_after_completion() {
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
        let (_backfill_tx, backfill_rx) = tokio::sync::mpsc::unbounded_channel();
        let (_forwarder_tx, forwarder_rx) = tokio::sync::mpsc::unbounded_channel();
        let forwarder = spawn_update_forwarder(
            vec![(host_transport.clone(), None)],
            update_rx,
            timeline.clone(),
            terminal_sync.clone(),
            sync_config.clone(),
            backfill_rx,
            forwarder_rx,
            None,
            Arc::new(Mutex::new(Vec::new())),
            false,
        );

        tokio::task::spawn_blocking(move || {
            let mut view = ClientGrid::new(rows as usize, cols as usize);
            let mut saw_prompt = false;
            let mut foreground_complete = false;
            let mut recent_complete = false;
            let mut history_complete = false;

            while !(foreground_complete && recent_complete && history_complete) {
                let message = client_transport
                    .recv(StdDuration::from_secs(1))
                    .expect("handshake frame");
                match message.payload {
                    Payload::Binary(bytes) => {
                        match protocol::decode_host_frame_binary(&bytes).expect("host frame") {
                            WireHostFrame::Hello { .. } => {}
                            WireHostFrame::Grid { .. } => {}
                            WireHostFrame::Snapshot { lane, updates, .. } => {
                                if lane == WireLane::Foreground {
                                    for update in &updates {
                                        view.apply_update(update);
                                    }
                                    if view.contains_row("host%") {
                                        saw_prompt = true;
                                    }
                                }
                            }
                            WireHostFrame::SnapshotComplete { lane, .. } => match lane {
                                WireLane::Foreground => foreground_complete = true,
                                WireLane::Recent => recent_complete = true,
                                WireLane::History => history_complete = true,
                            },
                            WireHostFrame::Delta { .. }
                            | WireHostFrame::HistoryBackfill { .. }
                            | WireHostFrame::Heartbeat { .. }
                            | WireHostFrame::InputAck { .. }
                            | WireHostFrame::Cursor { .. }
                            | WireHostFrame::Shutdown => {}
                        }
                    }
                    Payload::Text(text) => {
                        let trimmed = text.trim();
                        if trimmed == "__ready__" || trimmed == "__offer_ready__" {
                            continue;
                        }
                        panic!("unexpected text payload during handshake: {trimmed}");
                    }
                }
            }

            assert!(saw_prompt, "foreground snapshot missing prompt");

            match client_transport.recv(StdDuration::from_millis(750)) {
                Err(TransportError::Timeout) => {}
                Ok(message) => panic!("unexpected post-handshake frame: {message:?}"),
                Err(err) => panic!("transport error while waiting for refresh: {err:?}"),
            }
        })
        .await
        .expect("handshake refresh assertion");

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
                features: 0,
            },
        );
        send_host_frame(
            host_transport.as_ref(),
            HostFrame::Grid {
                cols: cols as u32,
                history_rows: rows as u32,
                base_row: grid.row_offset(),
                viewport_rows: None,
            },
        );
        let mut cache = TransmitterCache::new();
        transmit_initial_snapshots(&host_transport, &mut synchronizer, &mut cache, subscription)
            .expect("transmit snapshots");

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

    #[test_timeout::tokio_timeout_test]
    async fn handshake_suppresses_viewport_height_even_with_history() {
        let viewport_rows = 24;
        let viewport_cols = 80;
        let grid = Arc::new(TerminalGrid::new(viewport_rows, viewport_cols));
        let style_id = grid.ensure_style_id(Style::default());
        let packed = TerminalGrid::pack_char_with_style('X', style_id);
        // Extend history beyond the viewport to mimic long-running sessions.
        for row in viewport_rows..(viewport_rows + 120) {
            grid.write_packed_cell_if_newer(row, 0, (row as Seq) + 1, packed)
                .expect("extend history row");
        }
        let total_rows = grid.rows();
        assert!(
            total_rows > viewport_rows,
            "expected history beyond viewport"
        );

        let sync_config = SyncConfig::default();
        let terminal_sync = Arc::new(TerminalSync::new(
            grid.clone(),
            Arc::new(NullTerminalDeltaStream),
            sync_config.clone(),
        ));

        let pair = TransportPair::new(TransportKind::Ipc);
        let host_transport: Arc<dyn Transport> = Arc::from(pair.server);
        let client_transport: Arc<dyn Transport> = Arc::from(pair.client);

        let subscription = SubscriptionId(99);
        let mut cache = TransmitterCache::new();
        let _handshake = initialize_transport_snapshot(
            &host_transport,
            subscription,
            &terminal_sync,
            &sync_config,
            &mut cache,
            cursor_sync_enabled(),
        )
        .expect("handshake");

        let mut advertised: Option<(Option<u32>, u32, u32, u64)> = None;
        for _ in 0..10 {
            match recv_host_frame(client_transport.as_ref(), StdDuration::from_millis(200)) {
                WireHostFrame::Grid {
                    viewport_rows,
                    cols,
                    history_rows,
                    base_row,
                } => {
                    advertised = Some((viewport_rows, cols, history_rows, base_row));
                    break;
                }
                WireHostFrame::Hello { .. }
                | WireHostFrame::Snapshot { .. }
                | WireHostFrame::SnapshotComplete { .. }
                | WireHostFrame::Delta { .. }
                | WireHostFrame::HistoryBackfill { .. }
                | WireHostFrame::Heartbeat { .. }
                | WireHostFrame::InputAck { .. }
                | WireHostFrame::Cursor { .. } => {
                    continue;
                }
                WireHostFrame::Shutdown => break,
            }
        }

        let (rows, cols, total, base_row) = advertised.expect("grid frame missing from handshake");
        assert!(rows.is_none(), "handshake should not include viewport rows");
        assert_eq!(cols as usize, viewport_cols, "handshake cols mismatch");
        assert_eq!(total as usize, total_rows, "handshake history mismatch");
        assert_eq!(base_row, grid.row_offset(), "handshake base row mismatch");
    }

    #[test_timeout::timeout]
    fn history_backfill_contains_line_text() {
        let rows = 200usize;
        let cols = 80usize;
        let grid = TerminalGrid::new(rows, cols);
        let style_id = grid.ensure_style_id(Style::default());

        for row in 0..150usize {
            let text = format!("Line {}: Test", row + 1);
            let seq = (row as Seq) + 1;
            for (col, ch) in text.chars().enumerate() {
                let packed = TerminalGrid::pack_char_with_style(ch, style_id);
                grid.write_packed_cell_if_newer(row, col, seq, packed)
                    .expect("write cell");
            }
        }

        let chunk = collect_backfill_chunk(&grid, 112, 24);
        assert!(
            chunk.delivered >= 24,
            "expected delivered rows, got {}",
            chunk.delivered
        );

        let mut cache = TransmitterCache::new();
        cache.reset(cols);
        let converted_batch = cache.apply_updates(&chunk.updates, false);
        let wire_updates = converted_batch.updates;

        let mut seen_rows = Vec::new();
        for update in wire_updates {
            if let WireUpdate::Row { row, cells, .. } = update {
                let text: String = cells
                    .iter()
                    .map(|cell| {
                        let packed = PackedCell::from_raw(*cell);
                        terminal::unpack_cell(packed).0
                    })
                    .collect();
                seen_rows.push((row, text.trim_end().to_string()));
            }
        }

        assert!(
            seen_rows.iter().any(|(_, text)| text == "Line 113: Test"),
            "expected row text in backfill, got {:?}",
            seen_rows
        );
    }

    #[test_timeout::timeout]
    fn history_backfill_skips_default_rows() {
        let grid = TerminalGrid::new(24, 80);
        let chunk = collect_backfill_chunk(&grid, 10, 5);
        assert_eq!(chunk.delivered, 0);
        let mut cache = TransmitterCache::new();
        cache.reset(80);
        let batch = cache.apply_updates(&chunk.updates, false);
        assert!(
            batch.updates.is_empty(),
            "expected no updates for default rows"
        );
    }
}

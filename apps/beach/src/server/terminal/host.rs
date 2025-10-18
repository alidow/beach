use crate::cache::Seq;
use crate::cache::terminal::TerminalGrid;
use crate::client::terminal::join::{kind_label, summarize_offers};
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
use crate::protocol::terminal::bootstrap;
use crate::protocol::{self, ClientFrame as WireClientFrame, HostFrame};
use crate::server::terminal::runtime::{
    MAX_PTY_COLS, MAX_PTY_ROWS, build_spawn_config, handle_viewport_command,
    spawn_local_resize_monitor,
};
use crate::server::terminal::{
    AlacrittyEmulator, LocalEcho, PtyProcess, PtyWriter, TerminalEmulator, TerminalRuntime,
};
use crate::session::terminal::authorization::{JoinAuthorizationMetadata, JoinAuthorizer};
use crate::session::terminal::tty::{HostInputGate, RawModeGuard};
use crate::session::{HostSession, SessionConfig, SessionHandle, SessionManager};
use crate::sync::SyncConfig;
use crate::sync::terminal::server_pipeline::{
    BackfillCommand, ForwardTransport, ForwarderCommand, MAX_BACKFILL_ROWS_PER_REQUEST,
    TimelineDeltaStream, client_frame_label, send_host_frame, spawn_update_forwarder,
};
use crate::sync::terminal::{TerminalDeltaStream, TerminalSync};
use crate::telemetry::logging as logctl;
use crate::terminal::cli::{BootstrapOutput, HostArgs};
use crate::terminal::config::cursor_sync_enabled;
use crate::terminal::error::CliError;
use crate::transport as transport_mod;
use crate::transport::terminal::negotiation::{
    HeartbeatPublisher, NegotiatedSingle, NegotiatedTransport, SharedTransport,
    TransportSupervisor, negotiate_transport,
};
use crate::transport::{Payload, Transport, TransportError, TransportKind};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{self, IsTerminal, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};
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
    let ignore_sighup = bootstrap_mode && args.bootstrap_survive_sighup;
    configure_bootstrap_signal_handling(ignore_sighup);
    let local_preview_requested = args.local_preview;
    let local_preview_enabled = local_preview_requested && !bootstrap_mode;
    if local_preview_requested && !local_preview_enabled {
        warn!("local preview disabled when bootstrap output is active");
    }
    let interactive = !bootstrap_mode && io::stdin().is_terminal() && io::stdout().is_terminal();

    let input_gate = if interactive {
        Some(Arc::new(HostInputGate::new()))
    } else {
        None
    };

    if args.legacy_allow_all_clients {
        debug!(
            "deprecated --allow-all-clients flag supplied; approval already disabled by default"
        );
    }

    let approval_requested = args.require_client_approval && !args.legacy_allow_all_clients;
    if approval_requested && bootstrap_mode {
        warn!(
            "client approval prompts unavailable when bootstrap output is active; auto-accepting clients"
        );
    } else if approval_requested && !interactive {
        warn!("client approval prompts require an interactive TTY; auto-accepting clients");
    }

    let require_client_approval = approval_requested && interactive && !bootstrap_mode;
    if require_client_approval {
        debug!("client authorization prompt enabled (manual approval mode)");
    } else {
        debug!("client authorization prompt disabled (auto-accept mode)");
    }

    let authorizer = Arc::new(if require_client_approval {
        let gate = input_gate
            .as_ref()
            .expect("interactive input gate must be present for prompts");
        JoinAuthorizer::interactive(Arc::clone(gate))
    } else {
        JoinAuthorizer::allow_all()
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

    let raw_guard = RawModeGuard::new(interactive);

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
        let server = McpServer::new(McpConfig {
            socket: resolved_socket.clone(),
            use_stdio: args.mcp_stdio,
            read_only: !args.mcp_allow_write,
            allow_write: args.mcp_allow_write,
            session_filter: Some(vec![session_id.clone()]),
        });
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
    let mut forward_transports: Vec<ForwardTransport> = Vec::new();

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

    {
        let mut guard = input_handles.lock().unwrap();
        for handle in guard.drain(..) {
            handle.join().ok();
        }
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
        println!("\n✅ command '{command_display}' completed");
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

#[allow(clippy::too_many_arguments)]
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

#[allow(clippy::too_many_arguments)]
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

#[allow(clippy::too_many_arguments)]
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
        while let Ok(accepted) = supervisor.next().await {
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
                            let bridge_handle =
                                spawn_webrtc_bridge(handle, mcp_transport, MCP_CHANNEL_LABEL);
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
            HeartbeatPublisher::new(shared_arc.clone(), None).spawn(Duration::from_secs(10), None);

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
    });
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

fn display_cmd(cmd: &[String]) -> String {
    let mut rendered = String::new();
    for item in cmd {
        if rendered.is_empty() {
            rendered.push_str(item);
            continue;
        }
        rendered.push(' ');
        if item.chars().any(char::is_whitespace) {
            write!(&mut rendered, "\"{item}\"").ok();
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
    use crate::client::terminal::join::interpret_session_target;
    use crate::model::terminal::diff::{
        CacheUpdate, CellWrite, HistoryTrim, RectFill, RowSnapshot, StyleDefinition,
    };
    use crate::protocol::terminal::bootstrap::BootstrapHandshake;
    use crate::protocol::{
        self, ClientFrame as WireClientFrame, HostFrame as WireHostFrame, Lane as WireLane,
        Update as WireUpdate,
    };
    use crate::session::TransportOffer;
    use crate::sync::terminal::NullTerminalDeltaStream;
    use crate::sync::terminal::server_pipeline::{
        TransmitterCache, collect_backfill_chunk, initialize_transport_snapshot,
        sync_config_to_wire, transmit_initial_snapshots,
    };
    use crate::sync::{ServerSynchronizer, SubscriptionId};
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
        let script = format!("Last login: today\n{payload}\n");

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
        let rendered = bootstrap::render_remote_command(&cmd, true);
        assert!(rendered.starts_with("nohup '"));
        assert!(rendered.contains("'/opt/beach nightly'"));
        assert!(rendered.contains("'--flag'"));
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
                        .eq(needle_chars.iter().copied());
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
        let url = format!("https://example.com/api/sessions/{id}/join");
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
            let mut view = ClientGrid::new(rows, cols);
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
            let label = row + 1;
            let text = format!("Line {label}: Test");
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
            "expected row text in backfill, got {seen_rows:?}"
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

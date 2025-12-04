use crate::auth;
use crate::cache::terminal::TerminalGrid;
use crate::client::terminal::join::{kind_label, summarize_offers};
use crate::client::terminal::{ClientError, TerminalClient};
use crate::mcp::{
    McpConfig, default_socket_path as mcp_default_socket_path,
    registry::{
        RegistryGuard as McpRegistryGuard, TerminalSession as McpTerminalSession,
        global_registry as mcp_global_registry,
    },
    server::{McpServer, McpServerHandle},
};
use crate::metrics;
use crate::model::terminal::CursorState;
use crate::model::terminal::diff::CacheUpdate;
use crate::protocol::terminal::bootstrap;
use crate::protocol::{self, HostFrame};
use crate::server::terminal::runtime::{
    MAX_PTY_COLS, MAX_PTY_ROWS, build_spawn_config, handle_viewport_command,
    spawn_local_resize_monitor,
};
use crate::server::terminal::{
    AlacrittyEmulator, LocalEcho, PtyProcess, PtyWriter, TerminalEmulator, TerminalRuntime,
};
use crate::session::terminal::authorization::JoinAuthorizer;
use crate::session::terminal::tty::{HostInputGate, RawModeGuard};
use crate::session::{HostSession, SessionConfig, SessionHandle, SessionManager, TransportOffer};
use crate::sync::SyncConfig;
use crate::sync::terminal::server_pipeline::{
    BackfillCommand, ForwardTransport, ForwarderCommand, TimelineDeltaStream, send_host_frame,
    spawn_update_forwarder,
};
use crate::sync::terminal::{TerminalDeltaStream, TerminalSync};
use crate::terminal::cli::{BootstrapOutput, HostArgs};
use crate::terminal::config::cursor_sync_enabled;
use crate::terminal::error::CliError;
use crate::transport as transport_mod;
use crate::transport::terminal::negotiation::{
    HeartbeatPublisher, NegotiatedTransport, SharedTransport, negotiate_transport,
};
use crate::transport::unified_bridge::UnifiedBuggyTransport;
use crate::transport::{Payload, Transport, TransportError, TransportKind};
use beach_buggy::{
    AckStatus as CtrlAckStatus, ActionAck as CtrlActionAck, ActionCommand as CtrlActionCommand,
    ManagerTransport,
};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{self, IsTerminal, Read, Write};
use std::process;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};
use tokio::sync::{
    RwLock as AsyncRwLock,
    mpsc::{self, UnboundedSender},
    oneshot,
};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::field::display;
use tracing::{debug, info, trace, warn};

pub(crate) const MCP_CHANNEL_LABEL: &str = "mcp-jsonrpc";
pub(crate) const MCP_CHANNEL_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const CONTROLLER_CHANNEL_LABEL: &str = "mgr-actions";
#[allow(dead_code)]
pub(crate) const MANAGER_CHANNEL_LABEL: &str = "beach-manager";
#[allow(dead_code)]
pub(crate) const LEGACY_CONTROLLER_CHANNEL_LABEL: &str = "pb-controller";

#[derive(Default, Clone)]
struct FastPathStateChannel;

#[derive(Clone)]
struct UnifiedManagerHandle {
    bridge: Arc<AsyncRwLock<Option<Arc<UnifiedBuggyTransport>>>>,
    accept_unified: bool,
    allow_http_fallback: bool,
}

impl UnifiedManagerHandle {
    fn new(accept_unified: bool, allow_http_fallback: bool) -> Self {
        Self {
            bridge: Arc::new(AsyncRwLock::new(None)),
            accept_unified,
            allow_http_fallback,
        }
    }

    fn prefers_unified(&self) -> bool {
        self.accept_unified
    }

    fn supports_legacy_fastpath(&self) -> bool {
        !self.accept_unified && self.allow_http_fallback
    }

    #[allow(dead_code)]
    fn set_bridge(&self, bridge: Arc<UnifiedBuggyTransport>) -> Result<(), TransportError> {
        let mut guard = futures::executor::block_on(self.bridge.write());
        *guard = Some(bridge);
        Ok(())
    }

    #[allow(dead_code)]
    fn bridge(&self) -> Option<Arc<UnifiedBuggyTransport>> {
        futures::executor::block_on(self.bridge.read()).clone()
    }
}

#[tracing::instrument(
    name = "beach::terminal::host::run",
    skip(args),
    fields(session_id = tracing::field::Empty, pid = tracing::field::Empty)
)]
pub async fn run(base_url: &str, args: HostArgs) -> Result<(), CliError> {
    let pid = process::id();
    tracing::Span::current().record("pid", &display(pid));
    let delay_ms = args.dev_offer_encryption_delay_ms;
    if let Some(ms) = delay_ms {
        info!(delay_ms = ms, "development encryption delay enabled");
    }
    let _encryption_delay_guard =
        transport_mod::webrtc::install_offer_encryption_delay(delay_ms.map(Duration::from_millis));
    let mut requires_token = auth::manager_requires_access_token(base_url);
    if auth::is_public_mode() {
        trace!(
            target = "controller.actions",
            "public mode: skipping session-server auth requirement"
        );
        requires_token = false;
    }
    let access_token = auth::maybe_access_token(None, requires_token)
        .await
        .map_err(|err| CliError::Auth(err.to_string()))?;
    if requires_token && access_token.is_none() {
        return Err(CliError::Auth(
            "This private beach requires authentication. Run `beach login` and try again.".into(),
        ));
    }

    let mut config = SessionConfig::new(base_url)?;
    if let Some(token) = access_token.clone() {
        config = config.with_bearer_token(Some(token));
    }
    let manager = SessionManager::new(config)?;
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
    unsafe {
        std::env::set_var("BEACH_SESSION_ID", &session_id);
    }
    let attach_state = Arc::new(ControllerAttachState::new());
    attach_state.mark_attached();
    let controller_ctx = Arc::new(ControllerActionContext::new(
        session_id.clone(),
        attach_state.clone(),
    ));
    tracing::Span::current().record("session_id", &display(&session_id));
    if bootstrap_mode {
        info!(
            session_id = %session_id,
            pid,
            wait_for_peer = args.wait,
            survive_sighup = args.bootstrap_survive_sighup,
            "bootstrap host starting"
        );
    } else {
        info!(
            session_id = %session_id,
            pid,
            wait_for_peer = args.wait,
            interactive,
            "terminal host starting"
        );
    }
    // Debug marker so we can confirm the CLI build includes recent changes.
    info!(
        session_id = %session_id,
        "PONG_DEBUG_MARKER: mgr-state build active"
    );
    info!(session_id = %session_id, "session registered");
    // Surface the advertised WebRTC offer metadata to aid role/negotiation debugging.
    for offer in hosted.offers() {
        if let TransportOffer::WebRtc { offer } = offer {
            info!(
                session_id = %session_id,
                offer = %offer.to_string(),
                "advertised webrtc offer"
            );
        }
    }
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
    let fast_path_state_channel = Arc::new(FastPathStateChannel::default());
    let unified_manager = UnifiedManagerHandle::new(true, false);
    let hint_keys: Vec<String> = session_handle.transport_hints().keys().cloned().collect();
    info!(
        target = "transport.extension",
        session_id = %session_id,
        hint_keys = ?hint_keys,
        prefers_unified = unified_manager.prefers_unified(),
        legacy_fastpath = unified_manager.supports_legacy_fastpath(),
        "initialized unified transport preferences"
    );
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

    let (forwarder_updates_tx, forwarder_updates_rx) = mpsc::unbounded_channel();
    let cursor_tracker: Arc<Mutex<Option<CursorState>>> = Arc::new(Mutex::new(None));
    let last_terminal_update = Arc::new(AtomicU64::new(now_millis()));

    let updates_forward_task = {
        let mut updates = updates;
        let cursor_tracker = Arc::clone(&cursor_tracker);
        let last_terminal_update = Arc::clone(&last_terminal_update);
        tokio::spawn(async move {
            while let Some(update) = updates.recv().await {
                if let CacheUpdate::Cursor(cursor) = &update {
                    *cursor_tracker.lock().unwrap() = Some(*cursor);
                }
                last_terminal_update.store(now_millis(), Ordering::Relaxed);
                if forwarder_updates_tx.send(update).is_err() {
                    break;
                }
            }
        })
    };

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
        if let Some(path) = resolved_socket.as_ref() {
            unsafe {
                std::env::set_var("BEACH_MCP_SOCKET", path.display().to_string());
            }
            if !bootstrap_mode {
                println!("🔌 MCP socket listening at {}", path.display());
            } else {
                info!(socket = %path.display(), "mcp socket ready");
            }
        } else {
            unsafe {
                std::env::remove_var("BEACH_MCP_SOCKET");
            }
        }
        Some(guard)
    } else {
        unsafe {
            std::env::remove_var("BEACH_MCP_SOCKET");
        }
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
                None,
                None,
                None,
                session_id.clone(),
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
        let handle = spawn_local_stdin_forwarder(writer.clone(), local_echo.clone(), Some(gate));
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

    info!(
        session_id = %session_id,
        "spawning webrtc acceptor (host offerer)"
    );
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
        Arc::clone(&controller_ctx),
        fast_path_state_channel.clone(),
        unified_manager.clone(),
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
        forwarder_updates_rx,
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

    updates_forward_task.abort();
    let _ = updates_forward_task.await;
    // Idle snapshot publisher removed; state mirrors flow over unified transport only.

    if let Err(err) = updates_task.await {
        warn!(
            target = "beach::terminal::host",
            session_id = %session_id,
            pid,
            error = %err,
            "update forwarder ended unexpectedly"
        );
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
        let mut stdout = io::stdout().lock();
        blank_line(&mut stdout);
        writeln_cleared(
            &mut stdout,
            format_args!("✅ command '{command_display}' completed"),
        );
        blank_line(&mut stdout);
        let _ = stdout.flush();
    }
    info!(session_id = %session_id, pid, "host command completed");
    Ok(())
}

#[derive(Clone)]
pub(crate) struct ControllerActionContext {
    session_id: String,
    #[allow(dead_code)]
    attach_state: Arc<ControllerAttachState>,
}

#[derive(Clone, Default)]
struct ControllerAttachState {
    attached: Arc<AtomicBool>,
}

#[allow(dead_code)]
impl ControllerAttachState {
    fn new() -> Self {
        Self {
            attached: Arc::new(AtomicBool::new(false)),
        }
    }

    #[allow(dead_code)]
    fn is_attached(&self) -> bool {
        self.attached.load(Ordering::SeqCst)
    }

    fn mark_attached(&self) {
        let _ = self.attached.swap(true, Ordering::SeqCst);
    }

    #[allow(dead_code)]
    async fn wait_for_attach(&self) {
        while !self.is_attached() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

#[allow(dead_code)]
impl ControllerActionContext {
    fn new(session_id: String, attach_state: Arc<ControllerAttachState>) -> Self {
        Self {
            session_id,
            attach_state,
        }
    }

    fn session_id(&self) -> &str {
        &self.session_id
    }

    #[allow(dead_code)]
    fn attach_state(&self) -> Arc<ControllerAttachState> {
        Arc::clone(&self.attach_state)
    }

    #[allow(dead_code)]
    fn manager_client(&self) -> Option<()> {
        None
    }
}

#[allow(dead_code)]
fn controller_action_bytes<'a>(action: &'a CtrlActionCommand) -> Result<&'a str, String> {
    if action.action_type.as_str() != "terminal_write" {
        return Err(format!("unsupported action type {}", action.action_type));
    }
    action
        .payload
        .get("bytes")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "terminal_write payload missing bytes".to_string())
}

#[allow(dead_code)]
fn action_preview(payload: &str) -> String {
    const MAX_PREVIEW: usize = 64;
    let mut clean = payload.replace('\n', "\\n");
    if clean.len() > MAX_PREVIEW {
        clean.truncate(MAX_PREVIEW);
        clean.push('…');
    }
    clean
}

#[allow(dead_code)]
fn send_controller_ack(
    transport: &Arc<dyn Transport>,
    action_id: &str,
    applied_at: SystemTime,
) -> Result<(), TransportError> {
    let ack = CtrlActionAck {
        id: action_id.to_string(),
        status: CtrlAckStatus::Ok,
        applied_at,
        latency_ms: None,
        error_code: None,
        error_message: None,
    };
    let payload = serde_json::to_vec(&ack).map_err(|err| {
        TransportError::Setup(format!("failed to serialize controller ack: {err}"))
    })?;
    match transport.send_namespaced("controller", "ack", &payload) {
        Ok(_) => Ok(()),
        Err(TransportError::Setup(_)) => {
            // Fallback for transports that do not support namespaced framing.
            send_host_frame(transport, HostFrame::InputAck { seq: 0 }).map(|_| ())
        }
        Err(err) => Err(err),
    }
}

#[allow(dead_code)]
fn spawn_action_consumer(
    ctx: Arc<ControllerActionContext>,
    manager_url: String,
    _auth: (),
    writer_for_actions: PtyWriter,
    transport_hints: Arc<AsyncRwLock<HashMap<String, Value>>>,
    fast_path_bearer: Option<String>,
) -> Option<tokio::task::JoinHandle<()>> {
    let _ = (writer_for_actions, transport_hints, fast_path_bearer);
    warn!(
        target = "controller.actions",
        session_id = %ctx.session_id(),
        manager = %manager_url,
        "http action consumer disabled; unified transport required"
    );
    None
}

#[allow(dead_code)]
fn spawn_unified_action_consumer(
    ctx: Arc<ControllerActionContext>,
    bridge: Arc<UnifiedBuggyTransport>,
    writer_for_actions: PtyWriter,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let session_for_actions = ctx.session_id().to_string();
        let attach_state = ctx.attach_state();
        if !attach_state.is_attached() {
            info!(
                target = "controller.actions",
                session_id = %session_for_actions,
                "waiting for manager attach before starting unified action consumer"
            );
            attach_state.wait_for_attach().await;
            info!(
                target = "controller.actions",
                session_id = %session_for_actions,
                "manager attach confirmed; unified action consumer starting"
            );
        }
        loop {
            match bridge.receive_actions(&session_for_actions).await {
                Ok(actions) if !actions.is_empty() => {
                    debug!(
                        target = "controller.actions",
                        session_id = %session_for_actions,
                        count = actions.len(),
                        "received unified controller actions"
                    );
                    let mut acks: Vec<CtrlActionAck> = Vec::with_capacity(actions.len());
                    for cmd in actions {
                        trace!(
                            target = "transport.extension",
                            session_id = %session_for_actions,
                            kind = "action",
                            action_id = %cmd.id,
                            preview = %controller_action_bytes(&cmd).ok().map(action_preview).unwrap_or_else(|| "<invalid>".into()),
                            "received fastpath action via extension"
                        );
                        metrics::EXTENSION_RECEIVED
                            .with_label_values(&["fastpath", "action", "host"])
                            .inc();
                        let mut status = CtrlAckStatus::Ok;
                        let mut error_message = None;
                        match controller_action_bytes(&cmd) {
                            Ok(bytes) => match writer_for_actions.write(bytes.as_bytes()) {
                                Ok(()) => {}
                                Err(err) => {
                                    warn!(
                                        target = "controller.actions",
                                        session_id = %session_for_actions,
                                        command_id = %cmd.id,
                                        error = %err,
                                        "pty write failed for extension action"
                                    );
                                    status = CtrlAckStatus::Rejected;
                                    error_message = Some(err.to_string());
                                }
                            },
                            Err(err) => {
                                warn!(
                                    target = "controller.actions",
                                    session_id = %session_for_actions,
                                    command_id = %cmd.id,
                                    error = %err,
                                    "unsupported controller action via extension"
                                );
                                status = CtrlAckStatus::Rejected;
                                error_message = Some(err);
                            }
                        }
                        acks.push(CtrlActionAck {
                            id: cmd.id.clone(),
                            status,
                            applied_at: SystemTime::now(),
                            latency_ms: None,
                            error_code: None,
                            error_message,
                        });
                    }
                    if !acks.is_empty() {
                        if let Err(err) =
                            bridge.ack_actions(&session_for_actions, acks.clone()).await
                        {
                            metrics::EXTENSION_FALLBACK
                                .with_label_values(&[
                                    "fastpath",
                                    "ack",
                                    "host",
                                    "unified_send_error",
                                ])
                                .inc();
                            warn!(
                                target = "transport.extension",
                                session_id = %session_for_actions,
                                error = %err,
                                "unified ack send failed"
                            );
                        } else {
                            metrics::EXTENSION_SENT
                                .with_label_values(&["fastpath", "ack", "host", "unified"])
                                .inc_by(acks.len() as u64);
                        }
                    }
                }
                Ok(_) => {
                    sleep(Duration::from_millis(50)).await;
                }
                Err(err) => {
                    warn!(
                        target = "controller.actions",
                        session_id = %session_for_actions,
                        error = %err,
                        "receive_actions via unified transport failed"
                    );
                    sleep(Duration::from_millis(250)).await;
                }
            }
        }
    })
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

#[allow(dead_code)]
fn manager_supports_extensions(hints: &HashMap<String, Value>) -> bool {
    if supports_extensions_namespace(hints) {
        return true;
    }
    // Honor legacy fast_path_webrtc presence as an affirmative signal.
    if hints.get("fast_path_webrtc").is_some() {
        return true;
    }
    // Default to true when hints are absent/unclear to preserve legacy behavior.
    true
}

#[allow(dead_code)]
fn supports_extensions_namespace(hints: &HashMap<String, Value>) -> bool {
    hints
        .get("extensions")
        .and_then(|value| value.as_object())
        .and_then(|obj| obj.get("namespaces"))
        .map(|namespaces| match namespaces {
            Value::Array(items) => items
                .iter()
                .any(|ns| matches!(ns.as_str(), Some("manager") | Some("fastpath"))),
            Value::String(single) => single == "manager" || single == "fastpath",
            _ => false,
        })
        .unwrap_or(false)
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

fn clear_line<W: Write>(out: &mut W) {
    let _ = out.write_all(b"\r\x1b[2K");
}

fn writeln_cleared(out: &mut io::StdoutLock<'_>, args: std::fmt::Arguments<'_>) {
    clear_line(out);
    let _ = out.write_fmt(args);
    let _ = out.write_all(b"\n");
}

fn blank_line(out: &mut io::StdoutLock<'_>) {
    let _ = out.write_all(b"\r\x1b[2K\n");
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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
            let _ = write!(&mut rendered, "\"{item}\"");
        } else {
            rendered.push_str(item);
        }
    }
    rendered
}

fn print_host_banner(
    session: &HostSession,
    base: &str,
    selected: TransportKind,
    mcp_enabled: bool,
) {
    let handle = session.handle();
    let mut stdout = io::stdout().lock();

    let _ = writeln!(&mut stdout, "🏖️  beach session ready!");
    let _ = writeln!(&mut stdout, "session id   : {}", handle.session_id);
    let _ = writeln!(&mut stdout, "share url    : {}", handle.session_url);
    let _ = writeln!(&mut stdout, "passcode     : {}", session.join_code());
    let _ = writeln!(
        &mut stdout,
        "share command:\n  beach --session-server {} join {} --passcode {}",
        base,
        handle.session_id,
        session.join_code()
    );
    let _ = writeln!(
        &mut stdout,
        "transports   : {}",
        summarize_offers(handle.offers())
    );
    let _ = writeln!(&mut stdout, "active       : {}", kind_label(selected));
    if mcp_enabled {
        let _ = writeln!(
            &mut stdout,
            "mcp bridge   : beach --session-server {} join {} --passcode {} --mcp",
            base,
            handle.session_id,
            session.join_code()
        );
    }
    let _ = stdout.flush();
}

fn spawn_input_listener(
    transport: Arc<dyn Transport>,
    writer: PtyWriter,
    process: Arc<PtyProcess>,
    emulator: Arc<Mutex<Box<dyn TerminalEmulator + Send>>>,
    grid: Arc<TerminalGrid>,
    _backfill_tx: UnboundedSender<BackfillCommand>,
    _forwarder_tx: Option<UnboundedSender<ForwarderCommand>>,
    client_label: Option<String>,
    client_peer_id: Option<String>,
    peer_session_id: Option<String>,
    gate: Option<Arc<HostInputGate>>,
    session_id: String,
    controller_ctx: Option<Arc<ControllerActionContext>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let transport_id = transport.id().0;
        let transport_kind = transport.kind();
        loop {
            if let Some(g) = &gate {
                g.wait_until_resumed();
            }
            match transport.recv(Duration::from_millis(250)) {
                Ok(message) => match message.payload {
                    Payload::Binary(bytes) => {
                        if let Ok(frame) = protocol::decode_client_frame_binary(&bytes) {
                            match frame {
                                protocol::ClientFrame::Input { seq: _, data } => {
                                    if let Some(g) = &gate {
                                        g.wait_until_resumed();
                                    }
                                    if writer.write(&data).is_err() {
                                        break;
                                    }
                                }
                                protocol::ClientFrame::Resize { cols, rows } => {
                                    let _ = process
                                        .resize(cols.min(MAX_PTY_COLS), rows.min(MAX_PTY_ROWS));
                                    if let Ok(mut guard) = emulator.lock() {
                                        guard.resize(rows as usize, cols as usize);
                                    }
                                }
                                protocol::ClientFrame::ViewportCommand { command } => {
                                    let _ = handle_viewport_command(
                                        command,
                                        &writer,
                                        transport_id,
                                        &transport_kind,
                                        &grid,
                                        &_forwarder_tx,
                                    );
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                },
                Err(TransportError::ChannelClosed) => break,
                Err(_) => continue,
            }
        }
        drop(controller_ctx);
        drop(client_label);
        drop(client_peer_id);
        drop(peer_session_id);
        drop(session_id);
    })
}

fn spawn_local_stdin_forwarder(
    writer: PtyWriter,
    local_echo: Arc<LocalEcho>,
    gate: Option<Arc<HostInputGate>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut stdin = io::stdin();
        let mut buf = [0u8; 1024];
        loop {
            if let Some(g) = &gate {
                g.wait_until_resumed();
            }
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let _ = writer.write(&buf[..n]);
                    local_echo.record_input(&buf[..n]);
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    })
}

fn spawn_webrtc_acceptor(
    session_id: String,
    session_handle: SessionHandle,
    join_code: Option<String>,
    writer: PtyWriter,
    _process_handle: Arc<PtyProcess>,
    _emulator_handle: Arc<Mutex<Box<dyn TerminalEmulator + Send>>>,
    _grid: Arc<TerminalGrid>,
    _backfill_tx: UnboundedSender<BackfillCommand>,
    _input_handles: Arc<Mutex<Vec<thread::JoinHandle<()>>>>,
    _forwarder_cmd_tx: UnboundedSender<ForwarderCommand>,
    transports: Arc<Mutex<Vec<Arc<SharedTransport>>>>,
    _authorizer: Arc<JoinAuthorizer>,
    _mcp_handle: Option<McpServerHandle>,
    _mcp_bridges: Arc<Mutex<Vec<JoinHandle<()>>>>,
    first_ready_tx: Option<oneshot::Sender<()>>,
    controller_ctx: Arc<ControllerActionContext>,
    _fast_path_state_channel: Arc<FastPathStateChannel>,
    unified_manager: UnifiedManagerHandle,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let passphrase = join_code.clone();
        info!(
            target = "beach::terminal::host",
            session_id = %session_id,
            passphrase_present = %passphrase.as_ref().map(|p| !p.trim().is_empty()).unwrap_or(false),
            "starting webrtc acceptor (host offerer)"
        );

        let negotiated = negotiate_transport(
            &session_handle,
            passphrase.as_deref(),
            Some("beach-host"),
            false,
            None,
        )
        .await;

        let Ok(NegotiatedTransport::WebRtcOfferer { connection, .. }) = negotiated else {
            warn!(
                target = "beach::terminal::host",
                session_id = %session_id,
                error = ?negotiated.err(),
                "webrtc offerer negotiation failed"
            );
            if let Some(tx) = first_ready_tx {
                let _ = tx.send(());
            }
            return;
        };

        let transport = connection.transport();
        let metadata = connection.metadata();
        let shared = Arc::new(SharedTransport::new(transport.clone(), metadata));
        {
            let mut guard = transports.lock().unwrap();
            guard.push(shared);
        }
        info!(
            target = "beach::terminal::host",
            session_id = %session_id,
            transport_id = %transport.id().0,
            kind = ?transport.kind(),
            "webrtc offerer transport established"
        );

        // Bridge unified bus to buggy for controller actions/acks.
        let bridge = Arc::new(UnifiedBuggyTransport::new(transport.clone()));
        if let Err(err) = unified_manager.set_bridge(bridge.clone()) {
            warn!(
                target = "beach::terminal::host",
                session_id = %session_id,
                error = %err,
                "failed to set unified bridge"
            );
        }
        let _ = spawn_unified_action_consumer(controller_ctx, bridge, writer);

        // Keep the transport warm with heartbeats to avoid idle timeouts.
        HeartbeatPublisher::new(transport, None).spawn(Duration::from_secs(15), None);

        if let Some(tx) = first_ready_tx {
            let _ = tx.send(());
        }
    })
}

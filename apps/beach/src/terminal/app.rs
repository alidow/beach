use crate::client::terminal::{ClientError, TerminalClient};
use crate::mcp::client_proxy::spawn_client_proxy;
use crate::mcp::default_socket_path as mcp_default_socket_path;
use crate::protocol::terminal::bootstrap::{self, BootstrapHandshake};
use crate::server::terminal::host;
use crate::session::terminal::tty::RawModeGuard;
use crate::session::{JoinedSession, SessionConfig, SessionManager, TransportOffer};
use crate::terminal::cli::{self, Command, HostArgs, JoinArgs, SshArgs};
use crate::terminal::error::CliError;
use crate::terminal::negotiation::{NegotiatedSingle, NegotiatedTransport, negotiate_transport};
use crate::transport::TransportKind;
use std::io::{self, IsTerminal, Write};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{ChildStderr, Command as TokioCommand};
use tokio::time::timeout;
use tracing::{debug, info, warn};
use url::Url;
use uuid::Uuid;

pub async fn run(cli: cli::Cli) -> Result<(), CliError> {
    let session_base = cli.session_server;

    match cli.command {
        Some(Command::Join(args)) => handle_join(&session_base, args).await,
        Some(Command::Ssh(args)) => handle_ssh(&session_base, args).await,
        Some(Command::Host(args)) => host::run(&session_base, args).await,
        None => host::run(&session_base, HostArgs::default()).await,
    }
}

async fn handle_ssh(base_url: &str, args: SshArgs) -> Result<(), CliError> {
    bootstrap::copy_binary_to_remote(&args).await?;

    let remote_args = bootstrap::remote_bootstrap_args(&args, base_url);
    let remote_command = bootstrap::render_remote_command(&remote_args);

    let mut command = TokioCommand::new(&args.ssh_binary);
    if !args.no_batch {
        command.arg("-o").arg("BatchMode=yes");
    }
    if args.request_tty {
        command.arg("-tt");
    } else {
        command.arg("-T");
    }
    for flag in &args.ssh_flag {
        command.arg(flag);
    }
    command.arg(&args.target);
    command.arg(&remote_command);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    info!(
        target = %args.target,
        ssh_binary = %args.ssh_binary,
        remote_command = %remote_command,
        "launching ssh bootstrap"
    );

    eprintln!(
        "[DEBUG] SSH command: {} -o BatchMode=yes -T {} {} '{}'",
        args.ssh_binary,
        args.ssh_flag.join(" "),
        args.target,
        remote_command
    );

    let mut child = command.spawn()?;
    let mut stderr_pipe = child.stderr.take();

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| CliError::BootstrapHandshake("ssh stdout unavailable".into()))?;
    let mut reader = BufReader::new(stdout);
    let mut captured_stdout = Vec::new();
    let timeout_secs = args.handshake_timeout.max(1);
    let timeout = Duration::from_secs(timeout_secs);

    let handshake =
        match bootstrap::read_bootstrap_handshake(&mut reader, &mut captured_stdout, timeout).await
        {
            Ok(handshake) => handshake,
            Err(err) => {
                let _ = child.start_kill();
                let stderr_lines = if let Some(stderr) = stderr_pipe.take() {
                    collect_child_stream(stderr).await
                } else {
                    Vec::new()
                };
                let _ = child.wait().await;
                let mut context = err.to_string();
                if !captured_stdout.is_empty() {
                    context = format!("{}; stdout: {}", context, captured_stdout.join(" | "));
                }
                if !stderr_lines.is_empty() {
                    context = format!("{}; stderr: {}", context, stderr_lines.join(" | "));
                }
                return Err(CliError::BootstrapHandshake(context));
            }
        };

    if handshake.schema != BootstrapHandshake::SCHEMA_VERSION {
        warn!(
            schema = handshake.schema,
            expected = BootstrapHandshake::SCHEMA_VERSION,
            "bootstrap schema mismatch"
        );
    }
    if let Some(warning) = &handshake.warning {
        warn!(message = warning.as_str(), "remote bootstrap warning");
    }

    if !captured_stdout.is_empty() {
        debug!(lines = ?captured_stdout, "ssh stdout before handshake");
    }

    let mut stdout_task = None;
    let mut stderr_task = None;
    let mut wait_task = None;

    if args.keep_ssh {
        stdout_task = Some(tokio::spawn(forward_child_lines(reader, "stdout")));
        if let Some(stderr) = stderr_pipe.take() {
            stderr_task = Some(tokio::spawn(forward_child_lines(
                BufReader::new(stderr),
                "stderr",
            )));
        }
        wait_task = Some(tokio::spawn(async move {
            match child.wait().await {
                Ok(status) => {
                    if status.success() {
                        info!(
                            status = %bootstrap::describe_exit_status(status),
                            "ssh control channel closed"
                        );
                    } else {
                        warn!(
                            status = %bootstrap::describe_exit_status(status),
                            "ssh control channel closed with error"
                        );
                    }
                }
                Err(err) => warn!(error = %err, "failed to await ssh control channel"),
            }
        }));
    } else {
        drop(reader);
        drop(stderr_pipe); // Drop stderr to avoid blocking on it
        if let Err(err) = child.start_kill() {
            warn!(error = %err, "failed to terminate ssh process after bootstrap");
        }
        match child.wait().await {
            Ok(status) if !status.success() => {
                warn!(
                    status = %bootstrap::describe_exit_status(status),
                    "ssh exited with non-zero status after bootstrap"
                );
            }
            Err(err) => warn!(error = %err, "failed to await ssh process"),
            _ => {}
        }
    }

    if args.keep_ssh {
        info!("leaving ssh control channel open; enable info logs to tail remote output");
    }

    println!(
        "🔗 Starting beach session {} (remote {})",
        handshake.session_id, args.target
    );

    let join_args = JoinArgs {
        target: handshake.session_id.clone(),
        passcode: Some(handshake.join_code.clone()),
        label: None,
        mcp: false,
    };

    let join_result = handle_join(handshake.session_server.as_str(), join_args).await;

    if let Some(task) = stdout_task {
        let _ = task.await;
    }
    if let Some(task) = stderr_task {
        let _ = task.await;
    }
    if let Some(task) = wait_task {
        let _ = task.await;
    }

    join_result
}

async fn handle_join(base_url: &str, args: JoinArgs) -> Result<(), CliError> {
    let JoinArgs {
        target,
        passcode,
        label,
        mcp,
    } = args;

    let (session_id, inferred_base) = interpret_session_target(&target)?;
    let base = inferred_base.unwrap_or_else(|| base_url.to_string());

    let manager = SessionManager::new(SessionConfig::new(&base)?)?;
    let passcode = match passcode {
        Some(code) => code,
        None => prompt_passcode()?,
    };

    let trimmed_pass = passcode.trim().to_string();
    let joined = manager
        .join(&session_id, trimmed_pass.as_str(), label.as_deref(), mcp)
        .await?;
    let negotiated = negotiate_transport(
        joined.handle(),
        Some(trimmed_pass.as_str()),
        label.as_deref(),
        mcp,
    )
    .await?;
    let (transport, webrtc_channels) = match negotiated {
        NegotiatedTransport::Single(NegotiatedSingle {
            transport,
            webrtc_channels,
        }) => (transport, webrtc_channels),
        NegotiatedTransport::WebRtcOfferer { .. } => {
            return Err(CliError::TransportNegotiation(
                "unexpected offerer transport while joining session".into(),
            ));
        }
    };
    let selected_kind = transport.kind();
    info!(session_id = %joined.session_id(), transport = ?selected_kind, "joined session");
    print_join_banner(&joined, selected_kind);

    if mcp {
        if let Some(channels) = webrtc_channels.clone() {
            let session_for_proxy = session_id.clone();
            let proxy_path = mcp_default_socket_path(&session_id);
            let channels_clone = channels.clone();
            tokio::spawn(async move {
                match timeout(
                    host::MCP_CHANNEL_TIMEOUT,
                    channels_clone.wait_for(host::MCP_CHANNEL_LABEL),
                )
                .await
                {
                    Ok(Ok(mcp_transport)) => {
                        println!("🔌 MCP proxy listening at {}", proxy_path.display());
                        debug!(
                            target = "mcp::proxy",
                            session_id = %session_for_proxy,
                            path = %proxy_path.display(),
                            "spawning mcp client proxy"
                        );
                        let proxy_handle = spawn_client_proxy(proxy_path.clone(), mcp_transport);
                        let _ = proxy_handle.await;
                    }
                    Ok(Err(err)) => {
                        warn!(
                            target = "mcp::proxy",
                            session_id = %session_for_proxy,
                            error = %err,
                            "failed waiting for mcp channel"
                        );
                    }
                    Err(_) => {
                        warn!(
                            target = "mcp::proxy",
                            session_id = %session_for_proxy,
                            timeout_secs = host::MCP_CHANNEL_TIMEOUT.as_secs(),
                            "timed out waiting for mcp channel"
                        );
                    }
                }
            });
        } else {
            warn!(
                target = "mcp::proxy",
                "mcp channel unavailable for this transport"
            );
        }
    }

    let client_transport = transport.clone();
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    tokio::task::spawn_blocking(move || {
        let _raw_guard = RawModeGuard::new(interactive);
        let client = TerminalClient::new(client_transport).with_predictive_input(interactive);
        match client.run() {
            Ok(()) | Err(ClientError::Shutdown) => {}
            Err(err) => eprintln!("⚠️  client error: {err}"),
        }
    })
    .await
    .map_err(|err| CliError::Runtime(err.to_string()))?;

    Ok(())
}

pub(crate) fn interpret_session_target(target: &str) -> Result<(String, Option<String>), CliError> {
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

pub(crate) fn session_id_from_url(url: &Url) -> Option<String> {
    let mut segments: Vec<_> = url
        .path_segments()
        .map(|segments| segments.filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
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

pub(crate) fn base_from_url(url: &Url) -> Option<String> {
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

async fn collect_child_stream(stream: ChildStderr) -> Vec<String> {
    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    let mut lines = Vec::new();
    loop {
        buf.clear();
        match reader.read_line(&mut buf).await {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = buf.trim_end_matches(['\n', '\r']);
                if !trimmed.is_empty() {
                    lines.push(trimmed.to_string());
                }
            }
            Err(err) => {
                warn!(error = %err, "failed to read ssh stderr");
                break;
            }
        }
    }
    lines
}

async fn forward_child_lines<R>(mut reader: BufReader<R>, stream: &'static str)
where
    R: AsyncRead + Unpin,
{
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = line.trim_end_matches(['\n', '\r']);
                if !trimmed.is_empty() {
                    info!(target: "beach::ssh", stream = stream, message = trimmed);
                }
            }
            Err(err) => {
                warn!(target: "beach::ssh", stream = stream, error = %err, "failed to read ssh output");
                break;
            }
        }
    }
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

pub(crate) fn summarize_offers(offers: &[TransportOffer]) -> String {
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

pub(crate) fn kind_label(kind: TransportKind) -> &'static str {
    bootstrap::transport_kind_label(kind)
}

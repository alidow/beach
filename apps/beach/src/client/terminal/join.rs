use super::{ClientError, TerminalClient};
use crate::mcp::client_proxy::spawn_client_proxy;
use crate::mcp::default_socket_path as mcp_default_socket_path;
use crate::protocol::terminal::bootstrap;
use crate::session::terminal::tty::RawModeGuard;
use crate::session::{JoinedSession, SessionConfig, SessionManager, TransportOffer};
use crate::terminal::cli::JoinArgs;
use crate::terminal::error::CliError;
use crate::transport::TransportKind;
use crate::transport::terminal::negotiation::{
    NegotiatedSingle, NegotiatedTransport, negotiate_transport,
};
use std::io::{self, IsTerminal, Write};
use tokio::time::timeout;
use tracing::{debug, info, warn};
use url::Url;
use uuid::Uuid;

pub async fn run(base_url: &str, args: JoinArgs) -> Result<(), CliError> {
    run_with_notify(base_url, args, None).await
}

pub async fn run_with_notify(
    base_url: &str,
    args: JoinArgs,
    connected_notify: Option<tokio::sync::oneshot::Sender<()>>,
) -> Result<(), CliError> {
    let JoinArgs {
        target,
        passcode,
        label,
        mcp,
        inject_latency,
    } = args;

    let (session_id, inferred_base) = interpret_session_target(&target)?;
    let base = inferred_base.unwrap_or_else(|| base_url.to_string());

    let manager = SessionManager::new(SessionConfig::new(&base)?)?;
    let passcode = match passcode {
        Some(code) => code,
        None => prompt_passcode()?,
    };

    let trimmed_pass = passcode.trim().to_ascii_uppercase();
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

    if let Some(tx) = connected_notify {
        let _ = tx.send(());
    }

    if mcp {
        if let Some(channels) = webrtc_channels.clone() {
            let session_for_proxy = session_id.clone();
            let proxy_path = mcp_default_socket_path(&session_id);
            let channels_clone = channels.clone();
            tokio::spawn(async move {
                match timeout(
                    crate::server::terminal::host::MCP_CHANNEL_TIMEOUT,
                    channels_clone.wait_for(crate::server::terminal::host::MCP_CHANNEL_LABEL),
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
                            timeout_secs = crate::server::terminal::host::MCP_CHANNEL_TIMEOUT
                                .as_secs(),
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
    let session_id_for_debug = session_id.clone();

    tokio::task::spawn_blocking(move || {
        use crate::debug::ipc::start_diagnostic_listener;
        use crate::debug::server::DiagnosticServer;

        let _raw_guard = RawModeGuard::new(interactive);

        // Start diagnostic server
        let (diagnostic_server, (request_tx, response_rx)) = DiagnosticServer::new();
        let _listener_handle =
            start_diagnostic_listener(session_id_for_debug.clone(), request_tx, response_rx);

        let mut client = TerminalClient::new(client_transport)
            .with_predictive_input(interactive)
            .with_diagnostic_server(diagnostic_server);

        if let Some(latency_ms) = inject_latency {
            client = client.with_injected_latency_ms(latency_ms);
        }

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

fn session_id_from_url(url: &Url) -> Option<String> {
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
    if trimmed.len() == 6 && trimmed.chars().all(|c| c.is_ascii_alphanumeric()) {
        Ok(trimmed.to_ascii_uppercase())
    } else {
        Err(CliError::MissingPasscode)
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
        TransportOffer::WebSocketFallback { .. } => "WebSocket (Fallback)",
        TransportOffer::Ipc => "IPC",
    }
}

pub(crate) fn kind_label(kind: TransportKind) -> &'static str {
    bootstrap::transport_kind_label(kind)
}

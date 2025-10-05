use crate::session::{HostSession, TransportOffer};
use crate::terminal::cli::SshArgs;
use crate::terminal::error::CliError;
use crate::transport::TransportKind;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufRead, AsyncBufReadExt};
use tokio::process::Command as TokioCommand;
use tracing::{debug, info};

pub fn emit_bootstrap_handshake(
    session: &HostSession,
    base: &str,
    selected: TransportKind,
    command: &[String],
    wait_for_peer: bool,
    mcp_enabled: bool,
) -> Result<(), CliError> {
    let handle = session.handle();
    let handshake = BootstrapHandshake::from_context(
        session.session_id(),
        session.join_code(),
        base,
        handle.offers(),
        selected,
        command,
        wait_for_peer,
        mcp_enabled,
    );
    let payload = serde_json::to_string(&handshake)
        .map_err(|err| CliError::BootstrapOutput(err.to_string()))?;
    println!("{payload}");
    std::io::stdout()
        .flush()
        .map_err(|err| CliError::BootstrapOutput(err.to_string()))?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapHandshake {
    pub schema: u32,
    pub session_id: String,
    pub join_code: String,
    pub session_server: String,
    pub active_transport: String,
    pub transports: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub preferred_transport: Option<String>,
    pub host_binary: String,
    pub host_version: String,
    pub timestamp: u64,
    pub command: Vec<String>,
    pub wait_for_peer: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub warning: Option<String>,
    pub mcp_enabled: bool,
}

impl BootstrapHandshake {
    pub const SCHEMA_VERSION: u32 = 2;

    #[allow(clippy::too_many_arguments)]
    pub fn from_context(
        session_id: &str,
        join_code: &str,
        base: &str,
        offers: &[TransportOffer],
        selected: TransportKind,
        command: &[String],
        wait_for_peer: bool,
        mcp_enabled: bool,
    ) -> Self {
        let transports: Vec<String> = offers
            .iter()
            .map(|offer| offer.label().to_string())
            .collect();
        let preferred_transport = offers.first().map(|offer| offer.label().to_string());
        let warning = if transports.is_empty() {
            Some("session server returned no transport offers".to_string())
        } else {
            None
        };
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| Duration::from_secs(0))
            .as_secs();
        let host_binary = std::env::args()
            .next()
            .and_then(|arg0| {
                std::path::Path::new(&arg0)
                    .file_name()
                    .and_then(|name| name.to_str().map(|s| s.to_string()))
            })
            .unwrap_or_else(|| "beach".to_string());

        Self {
            schema: Self::SCHEMA_VERSION,
            session_id: session_id.to_string(),
            join_code: join_code.to_string(),
            session_server: base.to_string(),
            active_transport: transport_kind_label(selected).to_string(),
            transports,
            preferred_transport,
            host_binary,
            host_version: env!("CARGO_PKG_VERSION").to_string(),
            timestamp,
            command: command.to_vec(),
            wait_for_peer,
            warning,
            mcp_enabled,
        }
    }
}

pub fn remote_bootstrap_args(args: &SshArgs, session_server: &str) -> Vec<String> {
    let mut command = vec![
        args.remote_path.clone(),
        "host".to_string(),
        "--bootstrap-output=json".to_string(),
    ];
    command.extend(["--session-server".to_string(), session_server.to_string()]);
    if !args.command.is_empty() {
        command.push("--".to_string());
        command.extend(args.command.clone());
    }
    command
}

pub fn scp_destination(target: &str, remote_path: &str) -> String {
    if remote_path.contains(':') {
        remote_path.to_string()
    } else {
        format!("{target}:{remote_path}")
    }
}

pub fn render_remote_command(remote_args: &[String]) -> String {
    let quoted: Vec<String> = remote_args.iter().map(|arg| shell_quote(arg)).collect();
    let body = quoted.join(" ");
    let temp_file = "/tmp/beach-bootstrap-$$.json";
    format!("nohup {body} >{temp_file} 2>&1 </dev/null & sleep 2 && cat {temp_file}")
}

pub fn resolve_local_binary_path(args: &SshArgs) -> Result<PathBuf, CliError> {
    let raw_path = if let Some(custom) = &args.copy_from {
        custom.clone()
    } else {
        std::env::current_exe().map_err(|err| {
            CliError::CopyBinary(format!("unable to determine current executable: {err}"))
        })?
    };

    let resolved = if raw_path.is_relative() {
        std::fs::canonicalize(&raw_path).unwrap_or(raw_path.clone())
    } else {
        raw_path.clone()
    };

    if !resolved.exists() {
        let resolved_display = resolved.display();
        return Err(CliError::CopyBinary(format!(
            "local binary '{resolved_display}' does not exist"
        )));
    }

    Ok(resolved)
}

pub async fn copy_binary_to_remote(args: &SshArgs) -> Result<(), CliError> {
    if !args.copy_binary {
        return Ok(());
    }

    let source_path = resolve_local_binary_path(args)?;
    let destination = scp_destination(&args.target, &args.remote_path);

    info!(
        source = %source_path.display(),
        destination = %destination,
        scp_binary = %args.scp_binary,
        "uploading beach binary to remote host"
    );

    let mut command = TokioCommand::new(&args.scp_binary);
    if !args.no_batch {
        command.arg("-o").arg("BatchMode=yes");
    }
    for flag in &args.ssh_flag {
        command.arg(flag);
    }
    command.arg(&source_path);
    command.arg(&destination);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    let output = command
        .output()
        .await
        .map_err(|err| CliError::CopyBinary(format!("failed to spawn scp: {err}")))?;

    if output.status.success() {
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        let stdout_trimmed = stdout_str.trim();
        if !stdout_trimmed.is_empty() {
            debug!(stdout = stdout_trimmed, "scp stdout");
        }

        let stderr_str = String::from_utf8_lossy(&output.stderr);
        let stderr_trimmed = stderr_str.trim();
        if !stderr_trimmed.is_empty() {
            debug!(stderr = stderr_trimmed, "scp stderr");
        }
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(CliError::CopyBinary(format!(
        "{} failed ({}): stdout='{}' stderr='{}'",
        args.scp_binary,
        describe_exit_status(output.status),
        stdout.trim(),
        stderr.trim()
    )))
}

pub fn shell_quote(raw: &str) -> String {
    if raw.is_empty() {
        return "''".to_string();
    }
    let mut quoted = String::with_capacity(raw.len() + 2);
    quoted.push('\'');
    for ch in raw.chars() {
        if ch == '\'' {
            quoted.push_str("'\"'\"'");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

pub async fn read_bootstrap_handshake<R>(
    reader: &mut R,
    captured: &mut Vec<String>,
    timeout: Duration,
) -> Result<BootstrapHandshake, CliError>
where
    R: AsyncBufRead + Unpin,
{
    let deadline = Instant::now() + timeout;
    let mut line = String::new();
    loop {
        line.clear();
        let now = Instant::now();
        if now >= deadline {
            return Err(CliError::BootstrapHandshake(format!(
                "timed out after {}s waiting for bootstrap handshake",
                timeout.as_secs()
            )));
        }
        let remaining = deadline.saturating_duration_since(now);
        let read = tokio::time::timeout(remaining, reader.read_line(&mut line))
            .await
            .map_err(|_| {
                CliError::BootstrapHandshake(format!(
                    "timed out after {}s waiting for bootstrap handshake",
                    timeout.as_secs()
                ))
            })??;
        if read == 0 {
            return Err(CliError::BootstrapHandshake(
                "ssh connection closed before bootstrap handshake".into(),
            ));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<BootstrapHandshake>(trimmed) {
            Ok(handshake) => return Ok(handshake),
            Err(parse_err) => {
                if captured.len() < 32 {
                    captured.push(trimmed.to_string());
                }
                debug!(line = trimmed, error = %parse_err, "ignoring non-handshake stdout");
            }
        }
    }
}

pub fn transport_kind_label(kind: TransportKind) -> &'static str {
    match kind {
        TransportKind::WebRtc => "WebRTC",
        TransportKind::WebSocket => "WebSocket",
        TransportKind::Ipc => "IPC",
    }
}
pub fn describe_exit_status(status: std::process::ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("exit code {code}");
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("signal {signal}");
        }
    }

    "unknown status".to_string()
}

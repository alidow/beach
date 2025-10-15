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
    // If remote_path is relative (doesn't start with / or ~), prefix with ./
    // so the shell can find it in the current directory
    let executable_path = if args.remote_path.starts_with('/') || args.remote_path.starts_with('~')
    {
        args.remote_path.clone()
    } else {
        format!("./{}", args.remote_path)
    };

    let mut command = vec![
        executable_path,
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

/// Detect the architecture of the remote machine
async fn detect_remote_architecture(args: &SshArgs) -> Result<String, CliError> {
    let mut command = TokioCommand::new(&args.ssh_binary);
    if !args.no_batch {
        command.arg("-o").arg("BatchMode=yes");
    }
    command.arg("-T");
    for flag in &args.ssh_flag {
        command.arg(flag);
    }
    command.arg(&args.target);
    command.arg("uname -m");
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    let output = command.output().await.map_err(|err| {
        CliError::CopyBinary(format!("failed to detect remote architecture: {err}"))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::CopyBinary(format!(
            "failed to detect remote architecture ({}): {}",
            describe_exit_status(output.status),
            stderr.trim()
        )));
    }

    let arch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    info!(architecture = %arch, "detected remote architecture");
    Ok(arch)
}

/// Check if beach binary exists on remote and get its version
async fn check_remote_beach_version(args: &SshArgs) -> Result<Option<String>, CliError> {
    let remote_binary = if args.remote_path.starts_with('/') || args.remote_path.starts_with('~') {
        args.remote_path.clone()
    } else {
        format!("./{}", args.remote_path)
    };

    let mut command = TokioCommand::new(&args.ssh_binary);
    if !args.no_batch {
        command.arg("-o").arg("BatchMode=yes");
    }
    command.arg("-T");
    for flag in &args.ssh_flag {
        command.arg(flag);
    }
    command.arg(&args.target);
    command.arg(format!(
        "{} --version 2>/dev/null || echo NOTFOUND",
        shell_quote(&remote_binary)
    ));
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    let output = command.output().await.map_err(|err| {
        CliError::CopyBinary(format!("failed to check remote beach version: {err}"))
    })?;

    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.contains("NOTFOUND") || stdout.is_empty() {
        info!("no beach binary found on remote");
        Ok(None)
    } else {
        info!(version = %stdout, "found beach binary on remote");
        Ok(Some(stdout))
    }
}

/// Map remote architecture to Rust target triple
fn architecture_to_target(arch: &str) -> Result<&'static str, CliError> {
    match arch {
        "x86_64" => Ok("x86_64-unknown-linux-musl"),
        "aarch64" | "arm64" => Ok("aarch64-unknown-linux-musl"),
        "armv7l" => Ok("armv7-unknown-linux-musleabihf"),
        _ => Err(CliError::CopyBinary(format!(
            "unsupported remote architecture: {}. Supported: x86_64, aarch64/arm64, armv7l",
            arch
        ))),
    }
}

/// Build beach binary for the specified target
async fn build_for_target(target: &str) -> Result<PathBuf, CliError> {
    info!(target = %target, "building beach binary for target");

    let mut command = TokioCommand::new("cargo");
    command.arg("build");
    command.arg("--release");
    command.arg("--target");
    command.arg(target);
    command.arg("-p");
    command.arg("beach-human");
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    let output = command
        .output()
        .await
        .map_err(|err| CliError::CopyBinary(format!("failed to spawn cargo build: {err}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let error_msg = if stderr.contains("target may not be installed")
            || stderr.contains("can't find crate")
        {
            format!(
                "cargo build failed - target '{}' may not be installed.\n\
                 Run: rustup target add {}\n\
                 Error: {}",
                target,
                target,
                stderr.trim()
            )
        } else {
            format!(
                "cargo build failed ({}): {}",
                describe_exit_status(output.status),
                stderr.trim()
            )
        };
        return Err(CliError::CopyBinary(error_msg));
    }

    // Construct the path to the built binary
    let workspace_root = std::env::current_dir()
        .map_err(|err| CliError::CopyBinary(format!("failed to get current directory: {err}")))?;
    let binary_path = workspace_root
        .join("target")
        .join(target)
        .join("release")
        .join("beach-human");

    if !binary_path.exists() {
        return Err(CliError::CopyBinary(format!(
            "built binary not found at: {}",
            binary_path.display()
        )));
    }

    info!(path = %binary_path.display(), "successfully built beach binary");
    Ok(binary_path)
}

pub async fn copy_binary_to_remote(args: &SshArgs) -> Result<(), CliError> {
    if !args.copy_binary {
        return Ok(());
    }

    // Check if remote already has the correct version
    let local_version = env!("CARGO_PKG_VERSION");
    if let Ok(Some(remote_version)) = check_remote_beach_version(args).await {
        if remote_version.contains(local_version) {
            info!(
                local_version = %local_version,
                remote_version = %remote_version,
                "remote beach binary is up to date, skipping copy"
            );
            return Ok(());
        } else {
            info!(
                local_version = %local_version,
                remote_version = %remote_version,
                "remote beach version mismatch, will copy new binary"
            );
        }
    }

    // Detect remote architecture and build for it
    let source_path = if args.copy_from.is_some() {
        // User provided explicit path, use it as-is
        resolve_local_binary_path(args)?
    } else {
        // Auto-detect architecture and build
        let remote_arch = detect_remote_architecture(args).await?;
        let target = architecture_to_target(&remote_arch)?;

        info!(
            remote_arch = %remote_arch,
            target = %target,
            "auto-building for remote architecture"
        );

        build_for_target(target).await?
    };
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

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::CopyBinary(format!(
            "{} failed ({}): stdout='{}' stderr='{}'",
            args.scp_binary,
            describe_exit_status(output.status),
            stdout.trim(),
            stderr.trim()
        )));
    }

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

    // Make the remote binary executable
    info!(
        target = %args.target,
        remote_path = %args.remote_path,
        "making remote binary executable"
    );

    let mut chmod_command = TokioCommand::new(&args.ssh_binary);
    if !args.no_batch {
        chmod_command.arg("-o").arg("BatchMode=yes");
    }
    chmod_command.arg("-T");
    for flag in &args.ssh_flag {
        chmod_command.arg(flag);
    }
    chmod_command.arg(&args.target);
    chmod_command.arg(format!("chmod +x {}", shell_quote(&args.remote_path)));
    chmod_command.stdin(std::process::Stdio::null());
    chmod_command.stdout(std::process::Stdio::piped());
    chmod_command.stderr(std::process::Stdio::piped());

    let chmod_output = chmod_command
        .output()
        .await
        .map_err(|err| CliError::CopyBinary(format!("failed to spawn ssh for chmod: {err}")))?;

    if !chmod_output.status.success() {
        let stdout = String::from_utf8_lossy(&chmod_output.stdout);
        let stderr = String::from_utf8_lossy(&chmod_output.stderr);
        return Err(CliError::CopyBinary(format!(
            "chmod +x failed ({}): stdout='{}' stderr='{}'",
            describe_exit_status(chmod_output.status),
            stdout.trim(),
            stderr.trim()
        )));
    }

    Ok(())
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

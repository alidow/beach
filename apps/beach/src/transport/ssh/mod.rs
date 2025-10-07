use crate::client::terminal::join;
use crate::protocol::terminal::bootstrap::{self, BootstrapHandshake};
use crate::terminal::cli::{JoinArgs, SshArgs};
use crate::terminal::error::CliError;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{ChildStderr, Command as TokioCommand};
use tracing::{debug, info, warn};
use url::Url;

pub async fn run(base_url: &str, args: SshArgs) -> Result<(), CliError> {
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
        drop(stderr_pipe);
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

    let join_url = build_join_url(&handshake.session_server, &handshake.session_id);

    println!(
        "🔗 Starting beach session {} (remote {})",
        handshake.session_id, args.target
    );
    println!("  passcode  : {}", handshake.join_code);
    println!("  join url  : {}", join_url);

    let join_args = JoinArgs {
        target: handshake.session_id.clone(),
        passcode: Some(handshake.join_code.clone()),
        label: None,
        mcp: false,
        inject_latency: None,
    };

    let join_result = join::run(handshake.session_server.as_str(), join_args).await;

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

fn build_join_url(base: &str, session_id: &str) -> String {
    Url::parse(base)
        .and_then(|parsed| parsed.join(&format!("sessions/{session_id}/join")))
        .map(|url| url.to_string())
        .unwrap_or_else(|_| {
            let trimmed = base.trim_end_matches('/');
            format!("{trimmed}/sessions/{session_id}/join")
        })
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

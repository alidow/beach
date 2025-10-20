pub mod validate;

use crate::client::terminal::join;
use crate::protocol::terminal::bootstrap::{self, BootstrapHandshake};
use crate::terminal::cli::{JoinArgs, SshArgs};
use crate::terminal::error::CliError;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child as TokioChild, ChildStderr, Command as TokioCommand};
use tracing::{debug, info, warn};
use url::Url;
use validate::{HeadlessOptions, log_report, run_headless_validation};

pub async fn run(base_url: &str, args: SshArgs) -> Result<(), CliError> {
    bootstrap::copy_binary_to_remote(&args).await?;

    let remote_args = bootstrap::remote_bootstrap_args(&args, base_url);
    let remote_command = bootstrap::render_remote_command(&remote_args, args.ssh_keep_host_running);

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
                let stdout_lines = captured_stdout.len();
                let stdout_bytes: usize = captured_stdout.iter().map(|line| line.len()).sum();
                let stderr_lines_count = stderr_lines.len();
                warn!(
                    target = "beach::ssh",
                    remote = %args.target,
                    remote_command = %remote_command,
                    handshake_timeout = timeout_secs,
                    stdout_lines,
                    stdout_bytes,
                    stderr_lines = stderr_lines_count,
                    failure = %context,
                    "bootstrap handshake failed"
                );
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

    // Start draining SSH stdout/stderr to avoid backpressure. If --keep-ssh is set, log lines at info.
    let log_streams = args.keep_ssh;
    let stdout_task = Some(tokio::spawn(forward_child_lines(
        reader,
        if log_streams { "stdout" } else { "stdout" },
    )));
    let stderr_task = if let Some(stderr) = stderr_pipe.take() {
        Some(tokio::spawn(forward_child_lines(
            BufReader::new(stderr),
            if log_streams { "stderr" } else { "stderr" },
        )))
    } else {
        None
    };
    // Keep the SSH control channel alive until we decide to close it (managed below).

    let join_url = build_join_url(&handshake.session_server, &handshake.session_id);

    println!(
        "🔗 Starting beach session {} (remote {})",
        handshake.session_id, args.target
    );
    println!("  passcode  : {}", handshake.join_code);
    println!("  join url  : {}", join_url);
    if let Some(role) = &handshake.webrtc_offer_role {
        println!("  host webrtc role (advertised): {}", role);
    }

    if args.headless {
        let options = HeadlessOptions {
            timeout: Duration::from_secs(args.headless_timeout.max(1)),
            require_snapshot: true,
        };
        println!(
            "⚙️  Running headless validation (timeout {}s)...",
            options.timeout.as_secs()
        );
        let validation_result = run_headless_validation(
            &handshake.session_server,
            &handshake.session_id,
            &handshake.join_code,
            options,
        )
        .await;

        // Close SSH channel now that validation is complete.
        terminate_child(&mut child, "after headless validation").await;

        if let Some(task) = stdout_task {
            let _ = task.await;
        }
        if let Some(task) = stderr_task {
            let _ = task.await;
        }

        match validation_result {
            Ok(report) => {
                log_report(&report);
                println!("✅ Headless validation succeeded ({report})");
                if args.ssh_keep_host_running {
                    println!(
                        "Remote host remains running on {}; attach later with:\n  beach join {} --passcode {}",
                        args.target, handshake.session_id, handshake.join_code
                    );
                }
                return Ok(());
            }
            Err(err) => {
                return Err(err);
            }
        }
    }

    let join_args = JoinArgs {
        target: handshake.session_id.clone(),
        passcode: Some(handshake.join_code.clone()),
        label: None,
        mcp: false,
        inject_latency: None,
        headless: false,
        headless_timeout: 30,
    };

    // If we are keeping the remote host running, we can drop SSH immediately.
    if args.ssh_keep_host_running {
        terminate_child(&mut child, "after bootstrap").await;
    }

    // Otherwise, keep SSH open until the WebRTC/WebSocket transport connects, then close it.
    use tokio::sync::oneshot;
    let (connected_tx, connected_rx) = oneshot::channel::<()>();
    let session_server_owned = handshake.session_server.clone();
    // Wait until we connect or the join task finishes early (error/exit), then close SSH.
    let join_task = tokio::spawn(async move {
        join::run_with_notify(&session_server_owned, join_args, Some(connected_tx)).await
    });

    let _ = connected_rx.await; // Either Ok(()) on connect or Err if join ended early

    // Close SSH channel now that transport is established (or join ended).
    // We cannot signal the child directly here (owned by wait_task). Abort wait and let it end.
    if !args.ssh_keep_host_running {
        terminate_child(&mut child, "after transport connect").await;
    }

    // Await the log drainers to finish after SSH is gone.
    if let Some(task) = stdout_task {
        let _ = task.await;
    }
    if let Some(task) = stderr_task {
        let _ = task.await;
    }

    // Now await the join task for the actual session lifecycle result.
    let join_result = match join_task.await {
        Ok(result) => result,
        Err(err) => Err(CliError::Runtime(format!("join task failed: {err}"))),
    };

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

async fn terminate_child(child: &mut TokioChild, context: &str) {
    match child.start_kill() {
        Ok(()) => {}
        Err(err) => {
            // If the process is already gone, ignore the error.
            if err.kind() != std::io::ErrorKind::InvalidInput {
                warn!(target = "beach::ssh", error = %err, context, "failed to terminate ssh process");
            }
        }
    }
    match child.wait().await {
        Ok(status) if !status.success() => {
            warn!(
                target = "beach::ssh",
                status = %bootstrap::describe_exit_status(status),
                context,
                "ssh exited with non-zero status"
            );
        }
        Err(err) => {
            warn!(
                target = "beach::ssh",
                error = %err,
                context,
                "failed to await ssh process"
            );
        }
        _ => {}
    }
}

use crate::auth;
use crate::mcp::client::McpClient;
use crate::mcp::default_socket_path;
use crate::mcp::terminal::{CONTROLLER_ACQUIRE, CONTROLLER_QUEUE_ACTIONS, CONTROLLER_RELEASE};
use crate::terminal::cli::ActionArgs;
use crate::terminal::error::CliError;
use beach_buggy::ActionCommand as CtrlActionCommand;
use reqwest::Client;
use serde_json::json;
use std::path::{Path, PathBuf};

const HTTP_FALLBACK_HINT: &str =
    "Provide --manager-url/--controller-token or configure PRIVATE_BEACH_MANAGER_URL";

pub async fn run(profile: Option<&str>, args: ActionArgs) -> Result<(), CliError> {
    if args.bytes.is_empty() {
        return Err(CliError::InvalidArgument(
            "at least one --bytes argument is required".into(),
        ));
    }
    let session_id = resolve_session_id(&args)?;
    let actions = parse_actions(&args.bytes)?;
    let trace_id = args.trace_id.clone();

    if !args.no_ipc {
        if let Some(socket) = resolve_socket(&args, &session_id) {
            match run_ipc(
                &socket,
                &session_id,
                &actions,
                trace_id.as_deref(),
                args.lease_reason.as_deref(),
            )
            .await
            {
                Ok(()) => {
                    println!(
                        "✅ queued {} action(s) via MCP controller bridge",
                        actions.len()
                    );
                    return Ok(());
                }
                Err(IpcAttemptError::Unsupported) => {
                    eprintln!(
                        "⚠️  MCP controller tools unavailable at {}; falling back to HTTP",
                        socket.display()
                    );
                }
                Err(IpcAttemptError::Failed(err)) => {
                    eprintln!(
                        "⚠️  MCP queue attempt failed ({}); falling back to HTTP",
                        err
                    );
                }
            }
        }
    }

    run_http(profile, &args, &session_id, &actions, trace_id.as_deref()).await
}

fn resolve_session_id(args: &ActionArgs) -> Result<String, CliError> {
    if let Some(value) = args.session_id.as_ref() {
        if !value.trim().is_empty() {
            return Ok(value.trim().to_string());
        }
    }
    std::env::var("BEACH_SESSION_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
        .ok_or_else(|| CliError::InvalidArgument("session id required".into()))
}

fn parse_actions(inputs: &[String]) -> Result<Vec<CtrlActionCommand>, CliError> {
    let mut actions = Vec::new();
    for raw in inputs {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed = if trimmed.starts_with('[') {
            serde_json::from_str::<Vec<CtrlActionCommand>>(trimmed)
                .map_err(|err| CliError::InvalidArgument(format!("invalid action array: {err}")))?
        } else {
            vec![
                serde_json::from_str::<CtrlActionCommand>(trimmed).map_err(|err| {
                    CliError::InvalidArgument(format!("invalid action payload: {err}"))
                })?,
            ]
        };
        actions.extend(parsed);
    }
    if actions.is_empty() {
        return Err(CliError::InvalidArgument(
            "no valid action payloads provided".into(),
        ));
    }
    Ok(actions)
}

fn resolve_socket(args: &ActionArgs, session_id: &str) -> Option<PathBuf> {
    if let Some(path) = args.socket.as_ref() {
        return Some(path.clone());
    }
    if !session_id.is_empty() {
        return Some(default_socket_path(session_id));
    }
    None
}

enum IpcAttemptError {
    Unsupported,
    Failed(String),
}

async fn run_ipc(
    socket: &Path,
    session_id: &str,
    actions: &[CtrlActionCommand],
    trace_id: Option<&str>,
    reason: Option<&str>,
) -> Result<(), IpcAttemptError> {
    let mut client = McpClient::connect(socket)
        .await
        .map_err(|err| IpcAttemptError::Failed(err.to_string()))?;
    client
        .initialize()
        .await
        .map_err(|err| IpcAttemptError::Failed(err.to_string()))?;
    if !controller_tools_available(&mut client).await? {
        return Err(IpcAttemptError::Unsupported);
    }
    let mut acquire_args = json!({ "session_id": session_id });
    if let Some(reason) = reason {
        acquire_args["reason"] = json!(reason);
    }
    let lease_resp = client
        .call_tool(CONTROLLER_ACQUIRE, acquire_args)
        .await
        .map_err(|err| IpcAttemptError::Failed(err.to_string()))?;
    let lease_id = lease_resp
        .get("lease_id")
        .and_then(|value| value.as_str())
        .ok_or_else(|| IpcAttemptError::Failed("lease_id missing".into()))?
        .to_string();
    let mut queue_args = json!({
        "session_id": session_id,
        "actions": actions,
    });
    if let Some(trace) = trace_id {
        queue_args["trace_id"] = json!(trace);
    }
    client
        .call_tool(CONTROLLER_QUEUE_ACTIONS, queue_args)
        .await
        .map_err(|err| IpcAttemptError::Failed(err.to_string()))?;
    let release_args = json!({
        "session_id": session_id,
        "lease_id": lease_id,
    });
    let _ = client.call_tool(CONTROLLER_RELEASE, release_args).await;
    Ok(())
}

async fn controller_tools_available(client: &mut McpClient) -> Result<bool, IpcAttemptError> {
    let tools = client
        .list_tools()
        .await
        .map_err(|err| IpcAttemptError::Failed(err.to_string()))?;
    let available = tools.iter().any(|entry| {
        entry.get("name").and_then(|value| value.as_str()) == Some(CONTROLLER_QUEUE_ACTIONS)
    });
    Ok(available)
}

async fn run_http(
    profile: Option<&str>,
    args: &ActionArgs,
    session_id: &str,
    actions: &[CtrlActionCommand],
    trace_id: Option<&str>,
) -> Result<(), CliError> {
    let manager_url = args
        .manager_url
        .as_ref()
        .map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty())
        .ok_or_else(|| CliError::InvalidArgument(HTTP_FALLBACK_HINT.into()))?;
    let controller_token = args
        .controller_token
        .as_ref()
        .map(|tok| tok.trim().to_string())
        .filter(|tok| !tok.is_empty())
        .ok_or_else(|| CliError::InvalidArgument("controller token required for HTTP".into()))?;
    let requires_token = auth::manager_requires_access_token(&manager_url);
    let bearer = if let Some(token) = args
        .manager_token
        .as_ref()
        .map(|tok| tok.trim().to_string())
        .filter(|tok| !tok.is_empty())
    {
        Some(token)
    } else if requires_token {
        auth::maybe_access_token(profile, true)
            .await
            .map_err(|err| CliError::Auth(err.to_string()))?
    } else {
        None
    };
    let client = Client::builder()
        .build()
        .map_err(|err| CliError::Runtime(err.to_string()))?;
    let url = format!(
        "{}/sessions/{}/actions",
        manager_url.trim_end_matches('/'),
        session_id
    );
    let mut request = client.post(url).json(&json!({
        "controller_token": controller_token,
        "actions": actions,
    }));
    if let Some(token) = bearer.as_deref() {
        request = request.bearer_auth(token);
    }
    if let Some(trace) = trace_id {
        request = request.header("X-Trace-Id", trace);
    }
    let response = request
        .send()
        .await
        .map_err(|err| CliError::Runtime(err.to_string()))?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(CliError::Runtime(format!("manager HTTP {status}: {text}")));
    }
    println!("✅ queued {} action(s) via HTTP fallback", actions.len());
    Ok(())
}

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::cache::GridCache;
use crate::mcp::auth::{LeaseInfo, LeaseManager, LeaseScope};
use crate::mcp::registry::{TerminalSession, global_registry};

pub const ACQUIRE_LEASE: &str = "beach.terminal.acquireLease";
pub const RELEASE_LEASE: &str = "beach.terminal.releaseLease";
pub const SEND_TEXT: &str = "beach.terminal.sendText";
pub const SEND_KEYS: &str = "beach.terminal.sendKeys";
pub const RESIZE: &str = "beach.terminal.resize";
pub const SET_VIEWPORT: &str = "beach.terminal.setViewport";
pub const REQUEST_HISTORY: &str = "beach.terminal.requestHistory";
pub const LIST_SESSIONS: &str = "beach.sessions.list";

#[derive(Clone, Debug, serde::Serialize)]
pub struct TerminalToolDescriptor {
    pub name: String,
    pub description: String,
    pub requires_lease: bool,
}

pub fn list_tools(read_only: bool) -> Vec<TerminalToolDescriptor> {
    let mut tools = vec![TerminalToolDescriptor {
        name: LIST_SESSIONS.to_string(),
        description: "List active beach sessions".to_string(),
        requires_lease: false,
    }];

    if read_only {
        return tools;
    }

    tools.extend([
        TerminalToolDescriptor {
            name: ACQUIRE_LEASE.to_string(),
            description: "Acquire an exclusive input lease".to_string(),
            requires_lease: false,
        },
        TerminalToolDescriptor {
            name: RELEASE_LEASE.to_string(),
            description: "Release a previously acquired lease".to_string(),
            requires_lease: false,
        },
        TerminalToolDescriptor {
            name: SEND_TEXT.to_string(),
            description: "Send literal text to the terminal".to_string(),
            requires_lease: true,
        },
        TerminalToolDescriptor {
            name: SEND_KEYS.to_string(),
            description: "Send structured key presses".to_string(),
            requires_lease: true,
        },
        TerminalToolDescriptor {
            name: RESIZE.to_string(),
            description: "Resize the PTY".to_string(),
            requires_lease: true,
        },
        TerminalToolDescriptor {
            name: SET_VIEWPORT.to_string(),
            description: "Hint preferred viewport".to_string(),
            requires_lease: false,
        },
        TerminalToolDescriptor {
            name: REQUEST_HISTORY.to_string(),
            description: "Trigger history backfill".to_string(),
            requires_lease: false,
        },
    ]);

    tools
}

pub struct SendTextRequest {
    pub session_id: String,
    pub text: String,
    pub lease_id: Option<Uuid>,
}

impl SendTextRequest {
    pub fn from_params(value: &Value) -> Result<Self> {
        #[derive(Deserialize)]
        struct Helper {
            session_id: String,
            text: String,
            lease_id: Option<String>,
        }
        let helper: Helper = serde_json::from_value(value.clone())?;
        let lease_id = match helper.lease_id {
            Some(ref s) => Some(parse_uuid(s)?),
            None => None,
        };
        Ok(Self {
            session_id: helper.session_id,
            text: helper.text,
            lease_id,
        })
    }
}

pub struct SendKeysRequest {
    pub session_id: String,
    pub keys: Vec<KeySpec>,
    pub lease_id: Option<Uuid>,
}

impl SendKeysRequest {
    pub fn from_params(value: &Value) -> Result<Self> {
        #[derive(Deserialize)]
        struct Helper {
            session_id: String,
            keys: Vec<KeySpec>,
            lease_id: Option<String>,
        }
        let helper: Helper = serde_json::from_value(value.clone())?;
        Ok(Self {
            session_id: helper.session_id,
            keys: helper.keys,
            lease_id: helper.lease_id.map(|s| parse_uuid(&s)).transpose()?,
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KeySpec {
    Char {
        ch: char,
        #[serde(default)]
        modifiers: Vec<KeyModifier>,
    },
    Named {
        name: String,
        #[serde(default)]
        modifiers: Vec<KeyModifier>,
    },
    Raw {
        bytes: Vec<u8>,
    },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyModifier {
    Shift,
    Alt,
    Control,
    Super,
}

pub fn handle_send_text(
    session: &Arc<TerminalSession>,
    request: &SendTextRequest,
    leases: &LeaseManager,
) -> Result<()> {
    ensure_session_match(session, &request.session_id)?;
    leases.validate(&request.session_id, LeaseScope::Input, request.lease_id)?;
    session
        .writer
        .write(request.text.as_bytes())
        .map_err(|err| anyhow!("write to PTY: {err}"))
}

pub fn handle_send_keys(
    session: &Arc<TerminalSession>,
    request: &SendKeysRequest,
    leases: &LeaseManager,
) -> Result<()> {
    ensure_session_match(session, &request.session_id)?;
    leases.validate(&request.session_id, LeaseScope::Input, request.lease_id)?;
    let mut buffer = Vec::new();
    for spec in &request.keys {
        match encode_key(spec) {
            Some(bytes) => buffer.extend(bytes),
            None => return Err(anyhow!("unsupported key spec")),
        }
    }
    if buffer.is_empty() {
        return Ok(());
    }
    session
        .writer
        .write(&buffer)
        .map_err(|err| anyhow!("write to PTY: {err}"))
}

pub fn handle_resize(
    session: &Arc<TerminalSession>,
    params: &Value,
    leases: &LeaseManager,
) -> Result<Value> {
    #[derive(Deserialize)]
    struct Helper {
        session_id: String,
        cols: u16,
        rows: u16,
        lease_id: Option<String>,
    }
    let helper: Helper = serde_json::from_value(params.clone())?;
    ensure_session_match(session, &helper.session_id)?;
    let lease_id = helper.lease_id.map(|s| parse_uuid(&s)).transpose()?;
    leases.validate(&helper.session_id, LeaseScope::Input, lease_id)?;
    session
        .process
        .resize(helper.cols, helper.rows)
        .map_err(|err| anyhow!("resize PTY: {err}"))?;
    Ok(json!({"status": "ok", "cols": helper.cols, "rows": helper.rows}))
}

pub fn handle_set_viewport(
    session: &Arc<TerminalSession>,
    params: &Value,
    _leases: &LeaseManager,
) -> Result<Value> {
    #[derive(Deserialize)]
    struct Helper {
        session_id: String,
        top: Option<u64>,
        rows: Option<usize>,
    }
    let helper: Helper = serde_json::from_value(params.clone())?;
    ensure_session_match(session, &helper.session_id)?;
    Ok(json!({"status": "ack", "top": helper.top, "rows": helper.rows}))
}

pub fn handle_request_history(
    session: &Arc<TerminalSession>,
    params: &Value,
    _leases: &LeaseManager,
) -> Result<Value> {
    #[derive(Deserialize)]
    struct Helper {
        session_id: String,
        start_row: Option<u64>,
        count: Option<u32>,
    }
    let helper: Helper = serde_json::from_value(params.clone())?;
    ensure_session_match(session, &helper.session_id)?;
    Ok(json!({
        "status": "queued",
        "start_row": helper.start_row,
        "count": helper.count,
    }))
}

pub fn handle_acquire_lease(
    leases: &LeaseManager,
    session_id: &str,
    params: &Value,
) -> Result<LeaseInfo> {
    #[derive(Deserialize)]
    struct Helper {
        #[serde(default = "default_ttl")]
        ttl_ms: u64,
        #[serde(default)]
        scope: Option<String>,
    }
    let helper: Helper = serde_json::from_value(params.clone())?;
    let scope = match helper.scope.as_deref() {
        None | Some("input") => LeaseScope::Input,
        Some(other) => return Err(anyhow!("unsupported lease scope: {other}")),
    };
    let ttl = Duration::from_millis(helper.ttl_ms.clamp(1000, 120_000));
    Ok(leases.acquire(session_id, scope, ttl)?)
}

pub fn handle_release_lease(leases: &LeaseManager, params: &Value) -> Result<()> {
    #[derive(Deserialize)]
    struct Helper {
        lease_id: String,
    }
    let helper: Helper = serde_json::from_value(params.clone())?;
    let lease_id = parse_uuid(&helper.lease_id)?;
    Ok(leases.release(lease_id)?)
}

pub fn handle_list_sessions(_leases: &LeaseManager, _params: &Value) -> Result<Value> {
    let registry = global_registry();
    let sessions = registry
        .list_terminal_sessions()
        .into_iter()
        .map(|session| {
            let grid = session.sync.grid().clone();
            let (rows, cols) = grid.dims();
            json!({
                "session_id": session.session_id,
                "rows": rows,
                "cols": cols,
                "first_row": grid.first_row_id(),
                "last_row": grid.last_row_id(),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({"sessions": sessions}))
}

fn ensure_session_match(session: &Arc<TerminalSession>, requested: &str) -> Result<()> {
    if session.session_id != requested {
        return Err(anyhow!("session mismatch"));
    }
    Ok(())
}

fn encode_key(spec: &KeySpec) -> Option<Vec<u8>> {
    match spec {
        KeySpec::Char { ch, modifiers } => encode_char_key(*ch, modifiers),
        KeySpec::Named { name, modifiers } => encode_named_key(name, modifiers),
        KeySpec::Raw { bytes } => Some(bytes.clone()),
    }
}

fn encode_char_key(ch: char, modifiers: &[KeyModifier]) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    let lower = ch.to_ascii_lowercase();
    let mut alt = false;
    let mut ctrl = false;
    for modifier in modifiers {
        match modifier {
            KeyModifier::Alt => alt = true,
            KeyModifier::Control => ctrl = true,
            KeyModifier::Shift => {}
            KeyModifier::Super => return None,
        }
    }
    if alt {
        bytes.push(0x1b);
    }
    if ctrl {
        if ('a'..='z').contains(&lower) {
            bytes.push((lower as u8 - b'a') + 1);
        } else {
            return None;
        }
    } else {
        let mut buf = [0u8; 4];
        let encoded = lower.encode_utf8(&mut buf);
        bytes.extend_from_slice(encoded.as_bytes());
    }
    Some(bytes)
}

fn encode_named_key(name: &str, modifiers: &[KeyModifier]) -> Option<Vec<u8>> {
    let mut sequence = match name.to_ascii_lowercase().as_str() {
        "enter" => vec![b'\n'],
        "tab" => vec![b'\t'],
        "backspace" => vec![0x7f],
        "escape" | "esc" => vec![0x1b],
        "up" => b"\x1b[A".to_vec(),
        "down" => b"\x1b[B".to_vec(),
        "right" => b"\x1b[C".to_vec(),
        "left" => b"\x1b[D".to_vec(),
        "home" => b"\x1b[H".to_vec(),
        "end" => b"\x1b[F".to_vec(),
        "pageup" => b"\x1b[5~".to_vec(),
        "pagedown" => b"\x1b[6~".to_vec(),
        "delete" => b"\x1b[3~".to_vec(),
        "insert" => b"\x1b[2~".to_vec(),
        _ => return None,
    };

    if modifiers.iter().any(|m| matches!(m, KeyModifier::Alt)) {
        let mut prefixed = vec![0x1b];
        prefixed.append(&mut sequence);
        sequence = prefixed;
    }

    Some(sequence)
}

fn parse_uuid(value: &str) -> Result<Uuid> {
    Uuid::parse_str(value).map_err(|err| anyhow!("invalid uuid: {err}"))
}

fn default_ttl() -> u64 {
    30_000
}

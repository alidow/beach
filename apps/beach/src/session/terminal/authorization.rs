use crate::session::terminal::tty::HostInputGate;
use crate::transport::TransportKind;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event as CEvent, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::sleep;
use tracing::{debug, info, warn};

#[derive(Clone, Debug)]
pub struct JoinAuthorizationMetadata {
    pub transport_kind: TransportKind,
    pub peer_id: Option<String>,
    pub handshake_id: Option<String>,
    pub description: Option<String>,
    pub label: Option<String>,
    pub remote_addr: Option<String>,
    pub metadata: HashMap<String, String>,
}

impl JoinAuthorizationMetadata {
    pub fn from_parts(
        transport_kind: TransportKind,
        peer_id: Option<String>,
        handshake_id: Option<String>,
        description: Option<String>,
        metadata: HashMap<String, String>,
    ) -> Self {
        let label = metadata.get("label").cloned();
        let remote_addr = metadata.get("remote_addr").cloned();
        Self {
            transport_kind,
            peer_id,
            handshake_id,
            description,
            label,
            remote_addr,
            metadata,
        }
    }

    pub fn synopsis(&self) -> String {
        let mut parts = Vec::new();
        parts.push(format!("transport: {:?}", self.transport_kind));
        if let Some(peer) = &self.peer_id {
            parts.push(format!("peer: {peer}"));
        }
        if let Some(handshake) = &self.handshake_id {
            parts.push(format!("handshake: {handshake}"));
        }
        if let Some(desc) = &self.description {
            parts.push(desc.clone());
        }
        if let Some(label) = &self.label {
            parts.push(format!("label: {label}"));
        }
        if let Some(addr) = &self.remote_addr {
            parts.push(format!("remote: {addr}"));
        }
        if let Some(mcp_flag) = self.metadata.get("mcp") {
            if mcp_flag == "true" {
                parts.push("mcp:yes".to_string());
            }
        }
        if let Some(peer_session) = self.metadata.get("peer_session_id") {
            parts.push(format!("peer_session: {peer_session}"));
        }
        parts.join(", ")
    }
}

pub struct JoinAuthorizer {
    inner: JoinAuthorizerInner,
}

enum JoinAuthorizerInner {
    AllowAll,
    Interactive(InteractiveAuthorizer),
}

struct InteractiveAuthorizer {
    gate: Arc<HostInputGate>,
    prompt_lock: AsyncMutex<()>,
}

impl JoinAuthorizer {
    pub fn allow_all() -> Self {
        Self {
            inner: JoinAuthorizerInner::AllowAll,
        }
    }

    pub fn interactive(gate: Arc<HostInputGate>) -> Self {
        Self {
            inner: JoinAuthorizerInner::Interactive(InteractiveAuthorizer {
                gate,
                prompt_lock: AsyncMutex::new(()),
            }),
        }
    }

    pub fn should_emit_pending_hint(&self) -> bool {
        matches!(self.inner, JoinAuthorizerInner::Interactive(_))
    }

    pub fn should_emit_auto_granted(&self) -> bool {
        matches!(self.inner, JoinAuthorizerInner::AllowAll)
    }

    pub fn gate(&self) -> Option<Arc<HostInputGate>> {
        match &self.inner {
            JoinAuthorizerInner::Interactive(inner) => Some(Arc::clone(&inner.gate)),
            _ => None,
        }
    }

    pub async fn authorize(&self, metadata: JoinAuthorizationMetadata) -> bool {
        match &self.inner {
            JoinAuthorizerInner::AllowAll => true,
            JoinAuthorizerInner::Interactive(inner) => inner.authorize(metadata).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::TransportKind;

    #[test]
    fn synopsis_includes_peer_session_id_when_present() {
        let mut metadata = HashMap::new();
        metadata.insert("peer_session_id".to_string(), "peer-123".to_string());
        let summary = JoinAuthorizationMetadata::from_parts(
            TransportKind::WebRtc,
            Some("peer-a".into()),
            Some("handshake-1".into()),
            None,
            metadata,
        )
        .synopsis();
        assert!(
            summary.contains("peer_session: peer-123"),
            "peer_session_id should be included in synopsis: {}",
            summary
        );
    }
}

impl InteractiveAuthorizer {
    async fn authorize(&self, metadata: JoinAuthorizationMetadata) -> bool {
        let _guard = self.prompt_lock.lock().await;
        self.gate.pause();
        sleep(Duration::from_millis(50)).await;
        let mut decision = false;
        let prompt_metadata = metadata.clone();
        debug!(
            target = "host::auth",
            details = %prompt_metadata.synopsis(),
            "prompting for client authorization"
        );
        let gate = Arc::clone(&self.gate);
        let prompt_result =
            tokio::task::spawn_blocking(move || run_authorization_prompt(&prompt_metadata)).await;

        match prompt_result {
            Ok(Ok(allow)) => {
                decision = allow;
                if allow {
                    info!(
                        target = "host::auth",
                        details = %metadata.synopsis(),
                        "client authorized"
                    );
                } else {
                    info!(
                        target = "host::auth",
                        details = %metadata.synopsis(),
                        "client denied"
                    );
                }
            }
            Ok(Err(err)) => {
                warn!(
                    target = "host::auth",
                    error = %err,
                    details = %metadata.synopsis(),
                    "authorization prompt failed; denying client"
                );
            }
            Err(join_err) => {
                warn!(
                    target = "host::auth",
                    error = %join_err,
                    details = %metadata.synopsis(),
                    "authorization prompt task panicked; denying client"
                );
            }
        }

        let dropped = gate.resume_and_discard();
        if dropped > 0 {
            debug!(
                target = "host::auth",
                dropped_bytes = dropped,
                "discarded buffered stdin bytes after prompt"
            );
        }
        decision
    }
}

fn run_authorization_prompt(metadata: &JoinAuthorizationMetadata) -> io::Result<bool> {
    let raw_was_enabled = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if !raw_was_enabled {
        enable_raw_mode()?;
    }

    let mut stdout = io::stdout();
    let mut cleanup = PromptCleanup::new(raw_was_enabled);
    execute!(stdout, EnterAlternateScreen, Clear(ClearType::All), Hide)?;
    while event::poll(Duration::from_millis(0))? {
        let _ = event::read()?;
    }
    cleanup.alt_screen_active = true;
    write!(stdout, "\r==============================\r\n")?;
    write!(stdout, "\r  Incoming beach client join\r\n")?;
    write!(stdout, "\r==============================\r\n\r\n")?;
    write!(stdout, "\rtransport : {:?}\r\n", metadata.transport_kind)?;
    if let Some(desc) = &metadata.description {
        write!(stdout, "\rcontext   : {desc}\r\n")?;
    }
    if let Some(label) = &metadata.label {
        write!(stdout, "\rlabel     : {label}\r\n")?;
    }
    if let Some(peer) = &metadata.peer_id {
        write!(stdout, "\rpeer id   : {peer}\r\n")?;
    }
    if let Some(handshake) = &metadata.handshake_id {
        write!(stdout, "\rhandshake : {handshake}\r\n")?;
    }
    if let Some(remote) = &metadata.remote_addr {
        write!(stdout, "\rremote    : {remote}\r\n")?;
    }
    if !metadata.metadata.is_empty() {
        let mut extra: Vec<_> = metadata
            .metadata
            .iter()
            .filter(|(key, _)| key.as_str() != "label" && key.as_str() != "remote_addr")
            .collect();
        extra.sort_by(|a, b| a.0.cmp(b.0));
        for (key, value) in extra {
            write!(stdout, "\r{key}: {value}\r\n")?;
        }
    }
    write!(stdout, "\r\n")?;
    write!(
        stdout,
        "\rType 'yes' (enter) to allow, 'no' to deny. Press Ctrl+C to abort.\r\n\r\n"
    )?;
    stdout.flush()?;

    let mut decision: Option<bool> = None;
    let mut input = String::new();
    write!(stdout, "\rresponse  : ")?;
    stdout.flush()?;

    while decision.is_none() {
        if event::poll(Duration::from_millis(200))? {
            if let CEvent::Key(key) = event::read()? {
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(key.code, KeyCode::Char('c'))
                {
                    writeln!(stdout)?;
                    stdout.flush()?;
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "authorization aborted by user",
                    ));
                }

                match key.code {
                    KeyCode::Enter => {
                        let trimmed = input.trim().to_ascii_lowercase();
                        if trimmed.is_empty() {
                            write!(stdout, "\r\n")?;
                            write!(stdout, "\rPlease type 'yes' or 'no' and press enter.\r\n")?;
                            write!(stdout, "\rresponse  : {input}")?;
                            stdout.flush()?;
                            continue;
                        }
                        match trimmed.as_str() {
                            "yes" | "y" => decision = Some(true),
                            "no" | "n" => decision = Some(false),
                            _ => {
                                write!(stdout, "\r\n")?;
                                write!(
                                    stdout,
                                    "\rUnrecognized response '{trimmed}'. Type 'yes' or 'no'.\r\n"
                                )?;
                                write!(stdout, "\rresponse  : {input}")?;
                                stdout.flush()?;
                            }
                        }
                    }
                    KeyCode::Char(c)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        input.push(c);
                        write!(stdout, "{c}")?;
                        stdout.flush()?;
                    }
                    KeyCode::Backspace => {
                        if input.pop().is_some() {
                            write!(stdout, "\u{8} \u{8}")?;
                            stdout.flush()?;
                        }
                    }
                    KeyCode::Esc => {
                        decision = Some(false);
                    }
                    _ => {}
                }
            }
        }
    }

    write!(stdout, "\r\n")?;
    let allow = decision.unwrap_or(false);
    writeln!(
        stdout,
        "Decision recorded: {}",
        if allow { "allow" } else { "deny" }
    )?;
    stdout.flush()?;
    Ok(allow)
}

struct PromptCleanup {
    was_raw: bool,
    alt_screen_active: bool,
}

impl PromptCleanup {
    fn new(was_raw: bool) -> Self {
        Self {
            was_raw,
            alt_screen_active: false,
        }
    }
}

impl Drop for PromptCleanup {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        if self.alt_screen_active {
            let _ = execute!(stdout, LeaveAlternateScreen, Show);
        }
        let _ = stdout.flush();
        if !self.was_raw {
            let _ = disable_raw_mode();
        }
    }
}

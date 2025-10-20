use crate::protocol::{self, ClientFrame as WireClientFrame, HostFrame, Lane};
use crate::session::{SessionConfig, SessionManager};
use crate::terminal::error::CliError;
use crate::transport::terminal::negotiation::{self, NegotiatedTransport};
use crate::transport::{Payload, Transport, TransportError};
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

#[derive(Clone, Debug)]
pub struct HeadlessOptions {
    pub timeout: Duration,
    pub require_snapshot: bool,
}

impl Default for HeadlessOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            require_snapshot: true,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct ValidationReport {
    pub hello_seen: bool,
    pub grid_seen: bool,
    pub snapshot_rows: usize,
    pub snapshot_complete_lanes: Vec<Lane>,
    pub binary_frames: usize,
    pub text_messages: Vec<String>,
    pub duration: Duration,
}

impl ValidationReport {
    fn record_frame(&mut self, frame: &HostFrame) {
        self.binary_frames += 1;
        match frame {
            HostFrame::Hello { .. } => {
                self.hello_seen = true;
            }
            HostFrame::Grid { .. } => {
                self.grid_seen = true;
            }
            HostFrame::Snapshot { updates, .. } => {
                self.snapshot_rows += updates.len();
            }
            HostFrame::SnapshotComplete { lane, .. } => {
                if !self.snapshot_complete_lanes.contains(lane) {
                    self.snapshot_complete_lanes.push(*lane);
                }
            }
            HostFrame::Delta { .. }
            | HostFrame::Heartbeat { .. }
            | HostFrame::HistoryBackfill { .. }
            | HostFrame::InputAck { .. }
            | HostFrame::Cursor { .. }
            | HostFrame::Shutdown => {}
        }
    }

    fn stage(&self) -> &'static str {
        if self.snapshot_complete_lanes.contains(&Lane::Foreground) {
            "ready"
        } else if self.snapshot_rows > 0 {
            "snapshot"
        } else if self.grid_seen {
            "grid"
        } else if self.hello_seen {
            "hello"
        } else {
            "negotiation"
        }
    }

    fn is_success(&self, require_snapshot: bool) -> bool {
        if require_snapshot {
            self.snapshot_rows > 0 && self.snapshot_complete_lanes.contains(&Lane::Foreground)
        } else {
            self.hello_seen
        }
    }
}

impl fmt::Display for ValidationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "frames={}, rows={}, lanes={:?}, duration={}ms",
            self.binary_frames,
            self.snapshot_rows,
            self.snapshot_complete_lanes,
            self.duration.as_millis()
        )
    }
}

#[derive(Debug)]
enum ValidationFailure {
    Timeout { stage: &'static str },
    Transport(TransportError),
    Decode(String),
}

impl ValidationFailure {
    fn stage(&self) -> &'static str {
        match self {
            ValidationFailure::Timeout { stage } => stage,
            ValidationFailure::Transport(_) => "transport",
            ValidationFailure::Decode(_) => "decode",
        }
    }
}

impl fmt::Display for ValidationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationFailure::Timeout { stage } => {
                write!(f, "validation timed out while waiting for {stage}")
            }
            ValidationFailure::Transport(err) => write!(f, "transport error: {err}"),
            ValidationFailure::Decode(err) => write!(f, "failed to decode host frame: {err}"),
        }
    }
}

pub async fn run_headless_validation(
    base_url: &str,
    session_id: &str,
    passcode: &str,
    options: HeadlessOptions,
) -> Result<ValidationReport, CliError> {
    let manager = SessionManager::new(SessionConfig::new(base_url)?)?;
    let joined = manager
        .join(session_id, passcode, Some("headless-validator"), false)
        .await?;

    let negotiated = negotiation::negotiate_transport(
        joined.handle(),
        Some(passcode),
        Some("headless-validator"),
        false,
    )
    .await?;

    let transport = match negotiated {
        NegotiatedTransport::Single(single) => single.transport,
        NegotiatedTransport::WebRtcOfferer { .. } => {
            return Err(CliError::TransportNegotiation(
                "headless validator does not support offerer transports".into(),
            ));
        }
    };

    let transport_clone = Arc::clone(&transport);
    let timeout = options.timeout;
    let require_snapshot = options.require_snapshot;

    let report = match tokio::task::spawn_blocking(move || {
        execute_validation(transport_clone, timeout, require_snapshot)
    })
    .await
    .map_err(|err| CliError::Runtime(format!("validation task failed: {err}")))?
    {
        Ok(report) => report,
        Err(err) => {
            warn!(
                target = "beach::transport::ssh::validate",
                session_id = %session_id,
                error = %err,
                stage = %err.stage(),
                "headless validation failed"
            );
            return Err(CliError::TransportNegotiation(err.to_string()));
        }
    };

    info!(
        target = "beach::transport::ssh::validate",
        session_id = %session_id,
        snapshot_rows = report.snapshot_rows,
        lanes = ?report.snapshot_complete_lanes,
        binary_frames = report.binary_frames,
        duration_ms = report.duration.as_millis(),
        "headless validation succeeded"
    );

    Ok(report)
}

pub fn validate_existing_transport_blocking(
    transport: Arc<dyn Transport>,
    timeout: Duration,
    require_snapshot: bool,
) -> Result<ValidationReport, CliError> {
    match execute_validation(transport, timeout, require_snapshot) {
        Ok(report) => Ok(report),
        Err(err) => {
            warn!(
                target = "beach::transport::ssh::validate",
                stage = %err.stage(),
                error = %err,
                "headless validation failed"
            );
            Err(CliError::TransportNegotiation(err.to_string()))
        }
    }
}

fn execute_validation(
    transport: Arc<dyn Transport>,
    timeout: Duration,
    require_snapshot: bool,
) -> Result<ValidationReport, ValidationFailure> {
    send_ready_sentinel(transport.as_ref())?;
    send_initial_resize(transport.as_ref())?;
    let start = Instant::now();
    let mut report = run_validation_loop(transport, timeout, require_snapshot)?;
    report.duration = start.elapsed();
    Ok(report)
}

fn run_validation_loop(
    transport: Arc<dyn Transport>,
    timeout: Duration,
    require_snapshot: bool,
) -> Result<ValidationReport, ValidationFailure> {
    let mut report = ValidationReport::default();
    let deadline = Instant::now() + timeout;

    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(ValidationFailure::Timeout {
                stage: report.stage(),
            });
        }
        let wait = deadline
            .checked_duration_since(now)
            .unwrap_or_else(|| Duration::from_millis(1))
            .min(Duration::from_millis(500));

        match transport.recv(wait) {
            Ok(message) => match message.payload {
                Payload::Binary(bytes) => match protocol::decode_host_frame_binary(&bytes) {
                    Ok(frame) => {
                        debug!(
                            target = "beach::transport::ssh::validate",
                            frame = ?frame,
                            "received host frame"
                        );
                        report.record_frame(&frame);
                        if report.is_success(require_snapshot) {
                            return Ok(report);
                        }
                    }
                    Err(err) => {
                        return Err(ValidationFailure::Decode(err.to_string()));
                    }
                },
                Payload::Text(text) => {
                    let trimmed = text.trim().to_string();
                    debug!(
                        target = "beach::transport::ssh::validate",
                        payload = %trimmed,
                        "received text payload"
                    );
                    report.text_messages.push(trimmed);
                }
            },
            Err(TransportError::Timeout) => {
                continue;
            }
            Err(err) => {
                return Err(ValidationFailure::Transport(err));
            }
        }
    }
}

pub fn log_report(summary: &ValidationReport) {
    if !summary.text_messages.is_empty() {
        warn!(
            target = "beach::transport::ssh::validate",
            messages = ?summary.text_messages,
            "headless validation received unexpected text payloads"
        );
    }
}

fn send_initial_resize(transport: &dyn Transport) -> Result<(), ValidationFailure> {
    let resize = WireClientFrame::Resize { cols: 80, rows: 24 };
    let bytes = protocol::encode_client_frame_binary(&resize);
    transport
        .send_bytes(&bytes)
        .map(|_| ())
        .map_err(ValidationFailure::Transport)
}

fn send_ready_sentinel(transport: &dyn Transport) -> Result<(), ValidationFailure> {
    transport
        .send_text("__ready__")
        .map(|_| ())
        .map_err(ValidationFailure::Transport)
}

use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use once_cell::sync::Lazy;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Descriptor persisted alongside picker metadata so downstream components can
/// hydrate ScreenCaptureKit sessions or fall back when necessary.
#[derive(Clone, Debug)]
pub struct ScreenCaptureDescriptor {
    pub target_id: String,
    pub filter_blob: Vec<u8>,
    pub stream_config_blob: Option<Vec<u8>>,
    pub metadata_json: Option<String>,
}

impl ScreenCaptureDescriptor {
    pub fn new(
        target_id: impl Into<String>,
        filter_blob: Vec<u8>,
        stream_config_blob: Option<Vec<u8>>,
        metadata_json: Option<String>,
    ) -> Self {
        Self {
            target_id: target_id.into(),
            filter_blob,
            stream_config_blob,
            metadata_json,
        }
    }
}

/// Event emitted when the desktop picker confirms a target.
#[derive(Clone, Debug)]
pub struct SelectionEvent {
    pub descriptor: ScreenCaptureDescriptor,
    pub label: String,
    pub application: Option<String>,
    pub preview_path: Option<PathBuf>,
    pub confirmed_at: SystemTime,
}

impl SelectionEvent {
    pub fn new(
        descriptor: ScreenCaptureDescriptor,
        label: impl Into<String>,
        application: Option<String>,
        preview_path: Option<PathBuf>,
    ) -> Self {
        Self {
            descriptor,
            label: label.into(),
            application,
            preview_path,
            confirmed_at: SystemTime::now(),
        }
    }

    /// Helper primarily for tests that prefer a precise timestamp.
    #[allow(dead_code)]
    pub fn with_timestamp(
        descriptor: ScreenCaptureDescriptor,
        label: impl Into<String>,
        application: Option<String>,
        preview_path: Option<PathBuf>,
        confirmed_at: SystemTime,
    ) -> Self {
        Self {
            descriptor,
            label: label.into(),
            application,
            preview_path,
            confirmed_at,
        }
    }

    /// Milliseconds since the Unix epoch when the selection was confirmed.
    pub fn confirmed_at_millis(&self) -> u128 {
        self.confirmed_at
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    }
}

static SUBSCRIBERS: Lazy<Mutex<Vec<Sender<SelectionEvent>>>> =
    Lazy::new(|| Mutex::new(Vec::new()));
static LAST_SELECTION: Lazy<Mutex<Option<SelectionEvent>>> =
    Lazy::new(|| Mutex::new(None));

/// Subscribe to future desktop picker selections.
///
/// Each subscriber receives a clone of the [`SelectionEvent`]. Dropped receivers
/// are pruned automatically on the next publish.
pub fn subscribe_selection() -> Receiver<SelectionEvent> {
    let (tx, rx) = unbounded();
    SUBSCRIBERS
        .lock()
        .expect("selection subscribers mutex poisoned")
        .push(tx);
    rx
}

/// Returns the latest confirmed selection, if any.
pub fn last_selection() -> Option<SelectionEvent> {
    LAST_SELECTION
        .lock()
        .expect("selection cache mutex poisoned")
        .clone()
}

/// Publish a new selection event to all subscribers.
///
/// Returns the number of active subscribers that received the event.
pub fn publish_selection(event: SelectionEvent) -> usize {
    {
        let mut guard = LAST_SELECTION
            .lock()
            .expect("selection cache mutex poisoned");
        *guard = Some(event.clone());
    }

    let mut guard = SUBSCRIBERS
        .lock()
        .expect("selection subscribers mutex poisoned");
    let mut delivered = 0usize;
    guard.retain(|sender| {
        match sender.send(event.clone()) {
            Ok(()) => {
                delivered += 1;
                true
            }
            Err(_) => false,
        }
    });
    delivered
}

/// Block until a selection is available.
///
/// If a selection has already been published, it is returned immediately.
/// When `timeout` is `Some`, the function waits up to that duration for a new
/// selection; otherwise it waits indefinitely. Returns `None` if the timeout
/// elapses or all subscribers disconnect without producing a value.
pub fn wait_for_selection(timeout: Option<Duration>) -> Option<SelectionEvent> {
    if let Some(existing) = last_selection() {
        return Some(existing);
    }
    let rx = subscribe_selection();
    match timeout {
        Some(duration) => match rx.recv_timeout(duration) {
            Ok(event) => Some(event),
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => None,
        },
        None => rx.recv().ok(),
    }
}

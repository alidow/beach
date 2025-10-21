//! Cabana macOS native picker bridge facade.
//!
//! This crate provides an async-friendly API that wraps the native
//! `SCContentSharingPicker` once the Swift bridge is available.  For now
//! we expose a mock backend so the desktop shell can be developed on any
//! platform while the native bridge is implemented.

use std::{
    pin::Pin,
    sync::{Arc, Mutex},
};

use async_channel::{unbounded, Receiver as ChannelReceiver, Sender as ChannelSender};
use async_stream::stream;
use futures_core::stream::Stream;
use futures_util::stream::empty;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Shared result type for picker operations.
pub type Result<T, E = PickerError> = std::result::Result<T, E>;

/// Represents a capture target returned by the picker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PickerResult {
    pub id: String,
    pub label: String,
    pub application: Option<String>,
    pub kind: PickerItemKind,
    /// Serialized ScreenCaptureKit filter (macOS) or opaque payload for
    /// non-native backends.  The blob is opaque to consumers; the host runtime
    /// knows how to interpret it.
    pub filter_blob: Vec<u8>,
    /// Serialized `SCStreamConfiguration` describing capture parameters.
    pub stream_config_blob: Option<Vec<u8>>,
    /// Optional JSON metadata surfaced to the desktop UI for previews and
    /// telemetry.
    pub metadata_json: Option<String>,
}

/// Simplified classification so the UI can group windows vs displays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PickerItemKind {
    Window,
    Display,
    Application,
    Unknown,
}

/// Events that may be emitted by the picker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PickerEvent {
    Selection(PickerResult),
    Cancelled,
    Error { message: String },
}

/// Errors surfaced by picker operations.
#[derive(Debug, Error)]
pub enum PickerError {
    #[error("native picker is not available on this platform")]
    Unsupported,
    #[error("picker launch failed: {0}")]
    Launch(String),
    #[error("picker interaction failed: {0}")]
    Interaction(String),
}

/// Primary handle used by the desktop shell.
#[derive(Clone)]
pub struct PickerHandle {
    inner: Arc<Inner>,
}

enum Inner {
    #[cfg(all(target_os = "macos", feature = "native"))]
    Native(native::NativeHandle),
    #[cfg(feature = "mock")]
    Mock(MockHandle),
}

impl PickerHandle {
    /// Construct a picker handle.  On macOS with the `native` feature enabled we
    /// load the Swift bridge; otherwise we fall back to the mock backend when
    /// compiled with the `mock` feature.
    pub fn new() -> Result<Self> {
        #[cfg(all(target_os = "macos", feature = "native"))]
        {
            if let Ok(inner) = native::NativeHandle::new() {
                return Ok(Self {
                    inner: Arc::new(Inner::Native(inner)),
                });
            }
        }

        #[cfg(feature = "mock")]
        {
            return Ok(Self {
                inner: Arc::new(Inner::Mock(MockHandle::default())),
            });
        }

        #[cfg(not(feature = "mock"))]
        {
            return Err(PickerError::Unsupported);
        }
    }

    /// Explicitly request the mock backend for tests and non-macOS platforms.
    #[cfg(feature = "mock")]
    pub fn new_mock() -> Self {
        Self {
            inner: Arc::new(Inner::Mock(MockHandle::default())),
        }
    }

    /// Launch the picker UI.  For the mock backend this generates events; the
    /// native bridge will display the system picker.
    pub fn launch(&self) -> Result<()> {
        match &*self.inner {
            #[cfg(all(target_os = "macos", feature = "native"))]
            Inner::Native(handle) => handle.launch(),
            #[cfg(feature = "mock")]
            Inner::Mock(handle) => handle.launch(),
        }
    }

    /// Stop or hide the picker UI, allowing the caller to clean up resources.
    pub fn stop(&self) -> Result<()> {
        match &*self.inner {
            #[cfg(all(target_os = "macos", feature = "native"))]
            Inner::Native(handle) => handle.stop(),
            #[cfg(feature = "mock")]
            Inner::Mock(handle) => handle.stop(),
        }
    }

    /// Subscribe to picker events as an `impl Stream`.
    pub fn listen(&self) -> Pin<Box<dyn Stream<Item = PickerEvent> + Send>> {
        match &*self.inner {
            #[cfg(all(target_os = "macos", feature = "native"))]
            Inner::Native(handle) => handle.listen(),
            #[cfg(feature = "mock")]
            Inner::Mock(handle) => handle.listen(),
        }
    }
}

#[cfg(all(target_os = "macos", feature = "native"))]
mod native {
    use super::*;
    use async_channel::{unbounded, Receiver as ChannelReceiver, Sender as ChannelSender};
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use base64::Engine;
    use serde::Deserialize;
    use std::ffi::CStr;
    use std::os::raw::{c_char, c_void};
    use std::ptr::NonNull;

    type CabanaPickerCallback = unsafe extern "C" fn(*const c_char, *mut c_void);

    extern "C" {
        fn cabana_picker_is_available() -> bool;
        fn cabana_picker_create(
            callback: CabanaPickerCallback,
            user_data: *mut c_void,
            error_message: *mut *const c_char,
        ) -> *mut c_void;
        fn cabana_picker_present(handle: *mut c_void, error_message: *mut *const c_char) -> bool;
        fn cabana_picker_cancel(handle: *mut c_void);
        fn cabana_picker_destroy(handle: *mut c_void);
        fn cabana_picker_free_c_string(ptr: *const c_char);
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "lowercase")]
    enum BridgeEventType {
        Selection,
        Cancelled,
        Error,
    }

    #[derive(Deserialize)]
    struct BridgeEvent {
        #[serde(rename = "type")]
        event_type: BridgeEventType,
        id: Option<String>,
        label: Option<String>,
        application: Option<String>,
        kind: Option<String>,
        filter: Option<String>,
        #[serde(default)]
        configuration: Option<String>,
        #[serde(default)]
        metadata: Option<serde_json::Value>,
        #[serde(default)]
        message: Option<String>,
    }

    struct CallbackContext {
        sender: ChannelSender<PickerEvent>,
    }

    unsafe extern "C" fn event_callback(json_ptr: *const c_char, user_data: *mut c_void) {
        if json_ptr.is_null() || user_data.is_null() {
            return;
        }

        let context = &*(user_data as *mut CallbackContext);
        let json_str = match CStr::from_ptr(json_ptr).to_str() {
            Ok(s) => s,
            Err(err) => {
                let _ = context.sender.try_send(PickerEvent::Error {
                    message: format!("invalid UTF-8 event from native picker: {err}"),
                });
                return;
            }
        };

        let event: BridgeEvent = match serde_json::from_str(json_str) {
            Ok(evt) => evt,
            Err(err) => {
                let _ = context.sender.try_send(PickerEvent::Error {
                    message: format!("failed to parse picker event: {err}"),
                });
                return;
            }
        };

        match event.event_type {
            BridgeEventType::Selection => {
                if let (Some(id), Some(label), Some(filter_base64)) =
                    (event.id, event.label, event.filter)
                {
                    let filter_blob = match BASE64_STANDARD.decode(filter_base64.as_bytes()) {
                        Ok(data) => data,
                        Err(err) => {
                            let _ = context.sender.try_send(PickerEvent::Error {
                                message: format!("failed to decode picker filter: {err}"),
                            });
                            return;
                        }
                    };

                    let stream_config_blob = event.configuration.and_then(|cfg| {
                        BASE64_STANDARD
                            .decode(cfg.as_bytes())
                            .map_err(|err| {
                                let _ = context.sender.try_send(PickerEvent::Error {
                                    message: format!(
                                        "failed to decode picker configuration: {err}"
                                    ),
                                });
                                err
                            })
                            .ok()
                    });

                    let metadata_json = event
                        .metadata
                        .and_then(|value| serde_json::to_string(&value).ok());

                    let result = PickerResult {
                        id,
                        label,
                        application: event.application,
                        kind: match event.kind.as_deref() {
                            Some("window") => PickerItemKind::Window,
                            Some("display") => PickerItemKind::Display,
                            Some("application") => PickerItemKind::Application,
                            _ => PickerItemKind::Unknown,
                        },
                        filter_blob,
                        stream_config_blob,
                        metadata_json,
                    };

                    let _ = context.sender.try_send(PickerEvent::Selection(result));
                } else {
                    let _ = context.sender.try_send(PickerEvent::Error {
                        message: "selection event missing required fields".into(),
                    });
                }
            }
            BridgeEventType::Cancelled => {
                let _ = context.sender.try_send(PickerEvent::Cancelled);
            }
            BridgeEventType::Error => {
                let message = event.message.unwrap_or_else(|| "picker error".into());
                let _ = context.sender.try_send(PickerEvent::Error { message });
            }
        }
    }

    pub struct NativeHandle {
        handle: NonNull<c_void>,
        receiver: ChannelReceiver<PickerEvent>,
        context: *mut CallbackContext,
    }

    impl NativeHandle {
        pub fn new() -> Result<Self> {
            if unsafe { !cabana_picker_is_available() } {
                return Err(PickerError::Unsupported);
            }

            let (sender, receiver) = unbounded();
            let context = Box::into_raw(Box::new(CallbackContext { sender }));

            let mut error_ptr: *const c_char = std::ptr::null();
            let handle_ptr = unsafe {
                cabana_picker_create(event_callback, context as *mut c_void, &mut error_ptr)
            };

            if handle_ptr.is_null() {
                let message = extract_error(error_ptr)
                    .unwrap_or_else(|| "failed to initialize native picker".to_string());
                unsafe {
                    drop(Box::from_raw(context));
                }
                return Err(PickerError::Launch(message));
            }

            Ok(Self {
                handle: unsafe { NonNull::new_unchecked(handle_ptr) },
                receiver,
                context,
            })
        }

        pub fn launch(&self) -> Result<()> {
            let mut error_ptr: *const c_char = std::ptr::null();
            let ok = unsafe { cabana_picker_present(self.handle.as_ptr(), &mut error_ptr) };
            if ok {
                return Ok(());
            }

            let message = extract_error(error_ptr)
                .unwrap_or_else(|| "failed to present native picker".to_string());
            Err(PickerError::Launch(message))
        }

        pub fn stop(&self) -> Result<()> {
            unsafe { cabana_picker_cancel(self.handle.as_ptr()) };
            Ok(())
        }

        pub fn listen(&self) -> Pin<Box<dyn Stream<Item = PickerEvent> + Send>> {
            let receiver = self.receiver.clone();
            Box::pin(stream! {
                while let Ok(event) = receiver.recv().await {
                    yield event;
                }
            })
        }
    }

    impl Drop for NativeHandle {
        fn drop(&mut self) {
            unsafe {
                cabana_picker_destroy(self.handle.as_ptr());
                if !self.context.is_null() {
                    drop(Box::from_raw(self.context));
                    self.context = std::ptr::null_mut();
                }
            }
        }
    }

    fn extract_error(ptr: *const c_char) -> Option<String> {
        if ptr.is_null() {
            return None;
        }
        let message = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
        unsafe {
            cabana_picker_free_c_string(ptr);
        }
        Some(message)
    }
}

#[cfg(feature = "mock")]
#[derive(Clone)]
struct MockHandle {
    state: Arc<MockState>,
    sender: ChannelSender<PickerEvent>,
    receiver: Arc<Mutex<Option<ChannelReceiver<PickerEvent>>>>,
}

#[cfg(feature = "mock")]
struct MockState {
    inner: Mutex<MockStateInner>,
}

#[cfg(feature = "mock")]
#[derive(Clone)]
struct MockPreset {
    items: Vec<PickerResult>,
}

#[cfg(feature = "mock")]
#[derive(Clone)]
struct MockStateInner {
    launched: bool,
    preset: MockPreset,
}

#[cfg(feature = "mock")]
static DEFAULT_PRESET: Lazy<MockPreset> = Lazy::new(|| MockPreset {
    items: vec![
        PickerResult {
            id: "display:primary".into(),
            label: "Built-in Retina Display".into(),
            application: Some("System Display".into()),
            kind: PickerItemKind::Display,
            filter_blob: serde_json::to_vec(&serde_json::json!({
                "kind": "display",
                "width": 2560,
                "height": 1600
            }))
            .unwrap_or_default(),
            stream_config_blob: None,
            metadata_json: Some(
                serde_json::json!({
                    "pixel_density": "retina",
                    "refresh_hz": 60
                })
                .to_string(),
            ),
        },
        PickerResult {
            id: "window:com.apple.TextEdit:42".into(),
            label: "Notes.txt — TextEdit".into(),
            application: Some("TextEdit".into()),
            kind: PickerItemKind::Window,
            filter_blob: serde_json::to_vec(&serde_json::json!({
                "kind": "window",
                "window_id": 42,
                "app": "com.apple.TextEdit"
            }))
            .unwrap_or_default(),
            stream_config_blob: None,
            metadata_json: Some(
                serde_json::json!({
                    "bundle_id": "com.apple.TextEdit",
                    "visibility": "hidden"
                })
                .to_string(),
            ),
        },
    ],
});

#[cfg(feature = "mock")]
impl Default for MockHandle {
    fn default() -> Self {
        let (sender, receiver) = unbounded::<PickerEvent>();
        let preset = DEFAULT_PRESET.clone();
        Self {
            state: Arc::new(MockState {
                inner: Mutex::new(MockStateInner {
                    launched: false,
                    preset,
                }),
            }),
            sender,
            receiver: Arc::new(Mutex::new(Some(receiver))),
        }
    }
}

#[cfg(feature = "mock")]
impl MockHandle {
    fn launch(&self) -> Result<()> {
        let mut guard = self
            .state
            .inner
            .lock()
            .expect("mock picker state poisoned");

        if guard.launched {
            return Ok(());
        }
        guard.launched = true;
        let preset = guard.preset.clone();
        drop(guard);

        let sender = self.sender.clone();
        std::thread::spawn(move || {
            for item in preset.items {
                if sender.try_send(PickerEvent::Selection(item)).is_err() {
                    break;
                }
            }
        });

        Ok(())
    }

    fn stop(&self) -> Result<()> {
        let mut guard = self
            .state
            .inner
            .lock()
            .expect("mock picker state poisoned");

        if !guard.launched {
            return Ok(());
        }
        guard.launched = false;
        drop(guard);

        let _ = self.sender.try_send(PickerEvent::Cancelled);
        Ok(())
    }

    fn listen(&self) -> Pin<Box<dyn Stream<Item = PickerEvent> + Send>> {
        let receiver = self
            .receiver
            .lock()
            .expect("mock picker receiver poisoned")
            .take();

        if let Some(receiver) = receiver {
            Box::pin(stream! {
                while let Ok(event) = receiver.recv().await {
                    yield event;
                }
            })
        } else {
            Box::pin(empty::<PickerEvent>())
        }
    }
}

#[cfg(all(test, feature = "mock"))]
mod tests {
    use super::*;
    use test_timeout::tokio_timeout_test;

    #[tokio_timeout_test]
    async fn mock_picker_emits_preset_items() {
        let picker = PickerHandle::new_mock();
        let events = picker.listen();
        picker.launch().expect("launch succeeds");
        let mut seen = Vec::new();

        use futures_util::StreamExt;
        futures_util::pin_mut!(events);

        for _ in 0..2 {
            if let Some(PickerEvent::Selection(item)) = events.next().await {
                seen.push(item.id);
            }
        }

        assert!(
            seen.contains(&"display:primary".to_string())
                && seen.contains(&"window:com.apple.TextEdit:42".to_string()),
            "expected mock preset identifiers, got {seen:?}"
        );

        picker.stop().expect("stop succeeds");
    }
}

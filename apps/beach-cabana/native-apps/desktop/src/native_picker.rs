use std::thread;

use cabana_macos_picker::{PickerEvent, PickerHandle};
use futures_util::StreamExt;

/// Launch the native picker mock/bridge in a background thread when the
/// `CABANA_NATIVE_PICKER_BOOTSTRAP=1` environment variable is set.  This keeps
/// the current egui UI intact while allowing developers to exercise the picker
/// facade and observe emitted events.
pub fn bootstrap() {
    let should_launch = std::env::var("CABANA_NATIVE_PICKER_BOOTSTRAP")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !should_launch {
        return;
    }

    thread::spawn(|| {
        let picker = match PickerHandle::new() {
            Ok(handle) => handle,
            Err(err) => {
                eprintln!("[cabana-picker] failed to construct picker handle: {err}");
                return;
            }
        };

        if let Err(err) = picker.launch() {
            eprintln!("[cabana-picker] picker launch failed: {err}");
            return;
        }

        let stream_handle = picker.clone();
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
        {
            Ok(rt) => rt,
            Err(err) => {
                eprintln!("[cabana-picker] tokio runtime init failed: {err}");
                return;
            }
        };

        runtime.block_on(async move {
            let events = stream_handle.listen();
            futures_util::pin_mut!(events);

            let mut seen = 0usize;
            while let Some(event) = events.next().await {
                log_event(&event);
                seen += 1;
                if matches!(event, PickerEvent::Cancelled) || seen >= 8 {
                    break;
                }
            }

            if let Err(err) = picker.stop() {
                eprintln!("[cabana-picker] picker stop failed: {err}");
            }
        });
    });
}

fn log_event(event: &PickerEvent) {
    match event {
        PickerEvent::Selection(result) => {
            eprintln!(
                "[cabana-picker] selected: {} ({:?})",
                result.label, result.kind
            );
        }
        PickerEvent::Cancelled => {
            eprintln!("[cabana-picker] picker cancelled");
        }
    }
}

# Beach Cabana – Phase 4 (Selection UX & Verification)

Status (2025-10-19)
- ✅ Desktop picker graduated from a terminal placeholder to a windowed experience with live previews, filter/search, and macOS permission guidance.
- ✅ CLI flows now launch the picker automatically when `--window-id` is omitted, track Screen Recording permissions, and block streaming until users confirm the Noise verification string.
- ✅ Viewer-side WebRTC runs surface the verification code and support an explicit “decline” path without crashing the session.

What landed
- Replaced the terminal placeholder at `apps/beach-cabana/native-apps/desktop` with an `eframe`/`egui` picker. The modal lists displays/windows, supports tab+filter UX, renders live PNG previews, and highlights macOS Screen Recording permissions with a one-click re-request button.
- Picker now emits confirmed selections through a host-side relay (`beach_cabana_host::desktop`), so forthcoming shells can subscribe and begin capture immediately; quick actions let operators copy the identifier or open the latest preview file before sharing.
- Extended CLI commands (`preview`, `stream`, `encode`, and `webrtc-host-run`) to open the same picker when `--window-id` is absent, making keyboard-only and flag-driven flows equivalent. Each flow calls into the platform-specific permission helper before capturing frames (macOS re-requests, Windows/Linux surface guidance).
- CLI now consumes the relay automatically: if a selection is available it is reused, and when `CABANA_PICKER_RELAY=1` (or `CABANA_PICKER_WAIT_MS` is set) the CLI waits for the desktop picker to confirm before falling back to the TUI.
- Hardened zero-trust UX: `webrtc_viewer_run` now prints the 6-digit Noise verification code, waits for the operator to confirm it matches the host, and returns a `NoiseDriverError::UserAborted` if declined so scripts can branch cleanly. Host-side confirmation remains in place before capture starts.
- Added shared helpers (`resolve_window_id`, `prompt_screen_recording_permission`) so future commands inherit the picker + permission workflow without duplicating logic.

How to run
- Launch desktop picker: \
  `cargo run --manifest-path apps/beach-cabana/native-apps/desktop/Cargo.toml`
- CLI picker (prints selected id and exits): \
  `cargo run --manifest-path apps/beach-cabana/cli/Cargo.toml -- pick`
- Preview without knowing the id up front: \
  `cargo run --manifest-path apps/beach-cabana/cli/Cargo.toml -- preview`
- WebRTC host flow with interactive selection and verification gate: \
  `cargo run --manifest-path apps/beach-cabana/cli/Cargo.toml --features webrtc -- webrtc-host-run --session-id demo --passcode secret --frames 60`
- Viewer run now prompts for verification acknowledgement before frames are written: \
  `cargo run --manifest-path apps/beach-cabana/cli/Cargo.toml --features webrtc -- webrtc-viewer-run --session-id demo --passcode secret --recv-frames 30`
- Relay-enabled example (CLI waits up to 1.5s for the desktop picker before launching the TUI): \
  `CABANA_PICKER_RELAY=1 cargo run --manifest-path apps/beach-cabana/cli/Cargo.toml -- preview`

Notes & gaps
- Desktop picker still relies on placeholder enumeration for Windows/Linux until their adapters land, but the UX now displays the right permission guidance so we can drop TODO callouts later.
- The preview pipeline consumes temporary PNGs from `preview_window`; large captures are downscaled to keep UI responsive, but we still queue a tempo preview on every selection change. The most recent preview is left on disk so the host shell (or the quick action) can reuse it.
- CLI flows accept cancellation gracefully (no frames captured, informative stderr) so higher-level scripts can loop until a selection is confirmed. Relay waits are opt-in and default to zero delay unless `CABANA_PICKER_RELAY`/`CABANA_PICKER_WAIT_MS` are set.

Next steps toward Phase 5
- Flesh out the Windows/Linux capture adapters so the placeholder enumeration can be replaced with real window/display lists, and connect the permission banner to actual capability checks.
- Update the desktop host shell to subscribe directly to the relay once GUI capture streaming is ready, piping selections into live sessions without touching the CLI.
- Move H.264 segmented playback workforward so the viewer can start exercising real-time video once the host delivers browser-ready media.

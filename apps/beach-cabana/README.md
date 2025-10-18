# Beach Cabana Workspace

This directory is a private, self-contained workspace for GUI streaming (Cabana).
It separates the reusable host/engine from thin app wrappers so we can share
code between the CLI and future native apps while keeping GUI internals private.

Layout
- host/ — Rust library crate (beach-cabana-host)
  - capture/: frame producers (CoreGraphics + SCK hooks), resize helpers
  - platform/: window/display enumeration, previews, OS bridges
  - encoder/: GIF and VideoToolbox encoders (macOS)
  - noise/, security/: zero-trust handshake and sealed signaling
  - webrtc/: Noise over WebRTC data channels (feature `webrtc`)
  - fixture/: local sealed-signaling fixture for demos
- cli/ — Binary crate (beach-cabana)
  - User-facing commands (list-windows, preview, stream, encode, start, fixture)
  - For WebRTC flows, enable feature `webrtc` (for local dev only)
- native-apps/
  - desktop/ — placeholder for a desktop picker (e.g., Tauri/SwiftUI shell)
  - mobile/ — placeholder for later

Build
- CLI without WebRTC: `cargo run --manifest-path apps/beach-cabana/cli/Cargo.toml -- list-windows`
- Enable ScreenCaptureKit path on macOS: add `--features cabana_sck` when building host/cli
- WebRTC demo (when wired): add `--features webrtc` to CLI

Notes
- The root Cargo workspace excludes Cabana; build it via `--manifest-path` from this folder.
- All GUI-specific code lives here to keep the open-source terminal stack separate.
- Phase 4 (Selection UX) will add a shared selection API in `host/` used by both CLI TUI and desktop pickers.


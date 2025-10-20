# Cabana Workspace Conventions

This private workspace hosts the GUI sharing stack (Cabana). It is intentionally minimal and split for reuse.

## Layout
- `host/` (lib): cross‑platform engine
  - `capture/` — frame producers and adapters
  - `platform/` — window/display enumeration, OS bridges
  - `encoder/` — GIF and platform HW encoders
  - `security/` — sealed signaling (Noise PSK material)
  - `noise/` — Noise transport + media framing
  - `webrtc/` — Noise over WebRTC data channels (feature `webrtc`)
  - `fixture/` — local sealed‑signaling demo server + client
- `cli/` (bin): thin CLI wrapper that calls `host` APIs
  - commands: list/preview/stream/encode/start/fixture
  - TUI seed: `pick` for interactive selection
- `native-apps/desktop/` (placeholder): desktop picker shell(s). Keep assets and toolchains contained here.

## Build
- Host lib: `cargo check --manifest-path apps/beach-cabana/host/Cargo.toml`
- CLI: `cargo run --manifest-path apps/beach-cabana/cli/Cargo.toml -- list-windows`
- Features:
  - `--features webrtc` enables WebRTC + Noise transport
  - `--features cabana_sck` enables macOS ScreenCaptureKit + VideoToolbox paths

## Feature flags
- `webrtc` lives in `host` and is re‑exported through `cli`.
- `cabana_sck` gates macOS SCK/VT code; OSS/CI builds should work without it.

## Source placement rules
- New platform code goes under `host/src/platform/<os>/...` and is imported via `host::platform`.
- Keep OS‑specific capture encoders in `host/src/encoder` behind `cfg` + feature flags.
- CLI should remain thin: no platform logic; call into `host`.
- Desktop shells live in `native-apps/desktop/` and call `host` through a small FFI/plugin boundary if needed.

## Git hygiene
- No `Cargo.lock` committed under `apps/beach-cabana/` (this sub‑workspace is private and excluded from the root workspace).
- Ignore build artifacts and frontend toolchains recursively (see repo `.gitignore`).
- Keep the root of `apps/beach-cabana/` minimal: only `host/`, `cli/`, `native-apps/`, `Cargo.toml`, and docs.

## Adding a platform
- Add `host/src/platform/<os>/...` and expose an `enumerate_windows()` + `preview_window()` pair.
- Introduce a capture producer if needed, following the macOS pattern.
- Gate new dependencies under `cfg(target_os)` + a feature if they are optional.

## UX integration
- Selection UX: use `host::platform` to source items; wire into CLI and desktop pickers.
- Post‑handshake: always surface `Noise` verification code and gate capture until confirmed.

## Security
- All signaling payloads must be sealed (`security::seal_signaling_payload`) before leaving the process.
- Derive Noise PSK from session material only; never log secrets.


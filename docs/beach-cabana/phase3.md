# Beach Cabana – Phase 3 (Zero‑Trust Signaling & Media)

Status (2025-10-20)
- Noise transport delivers encrypted media over real WebRTC data channels; verification codes surface on both peers and must be confirmed before streaming.
- `host_bootstrap` posts sealed offers to either Beach Road or the fixture server and polls Beach Road, fixture HTTP, or fixture disk directories for the viewer’s sealed answer.
- Viewer flows now emit sealed answers back to the chosen relay (Road or fixture) and require an operator confirmation callback before frames are written to disk.
- CLI ergonomics:
  - `webrtc-host-run` accepts `--fixture-url` / `--fixture-dir`, shows the TUI picker when no window id is provided, and blocks capture until the Noise code is approved.
  - `webrtc-viewer-run` prompts for confirmation of the verification string and aborts gracefully (`NoiseDriverError::UserAborted`) when the codes do not match.
- Local demo (`webrtc-local`) remains the quick regression path without external signaling.
- Beach-surfer auto-detects Cabana sessions, renders PNG streams today, and will consume fragmented-MP4 once the host switches away from Annex B payloads.

How to run
- Local Noise/WebRTC demo:
  - `cargo run --manifest-path apps/beach-cabana/cli/Cargo.toml --features webrtc -- webrtc-local --session-id demo --passcode secret`
- ScreenCaptureKit stream metrics (macOS):
  - `bash temp/cabana-phase2-smoke.sh` (adds frames to `temp/beach-cabana-smoke/frames`)

Host/Viewer pairing via sealed signaling (fixture-assisted)
- Start the fixture:
  - `cargo run --manifest-path apps/beach-cabana/cli/Cargo.toml -- fixture-serve --listen 127.0.0.1:8081 --storage-dir ./cabana-fixture`
- Host (generate offer, post to fixture, poll fixture dir/endpoint, then optionally stream over data channel):
  - `cargo run --manifest-path apps/beach-cabana/cli/Cargo.toml --features webrtc -- webrtc-host-run --session-id demo --passcode secret --fixture-url http://127.0.0.1:8081/signaling --fixture-dir ./cabana-fixture`
  - Streaming flags (macOS): add `--window-id display:<ID> --frames 60 --interval-ms 33 [--max-width 1280] --codec h264|gif`
    - `--codec gif` sends PNG frames (viewer renders live and also writes `frame_XXX.png` locally).
    - `--codec h264` currently emits Annex B chunks to disk (`out.h264`). Update Phase 4/5 work will convert this path to produce fragmented MP4 before sending to browsers.
- Viewer (unseal host offer, post sealed answer, confirm verification code, then receive frames):
  - `cargo run --manifest-path apps/beach-cabana/cli/Cargo.toml --features webrtc -- webrtc-viewer-run --session-id demo --passcode secret --host-envelope '<compact-string>' --fixture-url http://127.0.0.1:8081/signaling --recv-frames 60`
  - Saves PNG frames into `temp/cabana-viewer-<ts>` by default (override with `--output-dir`).

Notes
- Fixture demo uses full ICE (non-trickle); host waits for gathering to finish before sealing SDP.
- Host bootstrap automatically watches the fixture directory or polling endpoint; viewers return a sealed answer to the same destination and the CLI surfaces success/failure.
- Script helper: `bash temp/cabana-phase3-demo.sh [--stream] [--codec h264|png] [--frames N] [--interval-ms MS] [--max-width W] [--play]`

Beach Road flow (recommended)
- `beach-cabana road-create-session --road-url http://127.0.0.1:8080` (prints URL + join code).
- `beach-cabana road-join-session --road-url http://127.0.0.1:8080 --session-id <ID>`.
- Host:
  - `beach-cabana webrtc-host-run --session-id <ID> --passcode <secret> --road-url http://127.0.0.1:8080 --from-id host --to-id viewer [--window-id display:<ID> --codec h264|gif --frames 60 --interval-ms 33 --max-width 1280]`
- Viewer:
  - `beach-cabana webrtc-viewer-run --session-id <ID> --passcode <secret> --road-url http://127.0.0.1:8080 --from-id viewer --to-id host --recv-frames 60`

Next steps
- Media packaging: integrate a fragmented MP4 muxer (or WebRTC video track) so `--codec h264` produces browser-ready segments before transmission.
- Viewer polish (Phase 5): add playback controls, render latency/metrics overlays, and surface an explicit error when the host sends Annex B H.264 without packaging.
- Extend the confirmation UX into graphical shells (desktop picker) so verification happens on both CLI and GUI surfaces.

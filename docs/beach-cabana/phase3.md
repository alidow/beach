# Beach Cabana – Phase 3 (Zero‑Trust Signaling & Media)

Status (2025-10-18)
- Noise/transport complete: `NoiseController`, AEAD media frames, replay protection, verification code.
- Channel adapter landed: `NoiseDriver` + `webrtc` feature wires a real WebRTC data channel into the Noise transport.
- Local demo: `beach-cabana webrtc-local --session-id ... --passcode ...` creates two in‑process peers, opens a data channel, completes the Noise handshake, prints a verification code, and exchanges encrypted demo messages. This runs without external signaling and is suitable for quick validation.
- Sealed signaling now uses Beach Road endpoints end‑to‑end:
  - Host: POST `/sessions/{id}/webrtc/offer` with sealed payload, then GET `/sessions/{id}/webrtc/answer?handshake_id=...`
  - Viewer: GET `/sessions/{id}/webrtc/offer?peer_id=...`, then POST `/sessions/{id}/webrtc/answer` with sealed payload.
- New helpers to bootstrap sessions on Beach Road:
  - `road-create-session` → POST `/sessions` and print session URL + join code
  - `road-join-session` → POST `/sessions/{id}/join` (utility)
- Sealed signaling primitives exist (Phase 1) and remain the plan for offer/answer envelopes.

How to run
- Local E2E over data channel:
  - `cargo run --manifest-path apps/beach-cabana/Cargo.toml --features webrtc -- webrtc-local --session-id demo --passcode secret`
- ScreenCaptureKit stream metrics (macOS):
  - `bash temp/cabana-phase2-smoke.sh` (adds frames to `temp/beach-cabana-smoke/frames`)

Host/Viewer pairing via sealed signaling (fixture-assisted)
- Start the fixture:
  - `cargo run --manifest-path apps/beach-cabana/Cargo.toml -- fixture-serve --listen 127.0.0.1:8081 --storage-dir ./cabana-fixture`
- Host (generate offer, post to fixture, poll fixture dir, then complete and optionally stream over data channel):
  - `cargo run --manifest-path apps/beach-cabana/Cargo.toml --features webrtc -- webrtc-host-run --session-id demo --passcode secret --fixture-url http://127.0.0.1:8081/signaling --fixture-dir ./cabana-fixture`
  - To stream frames (macOS): add `--window-id display:<ID> --frames 60 --interval-ms 33 [--max-width 1280] --codec h264|gif`
    - `--codec h264` sends H.264 Annex B chunks (viewer writes `out.h264`)
    - `--codec gif` sends PNG frames (viewer writes `frame_XXX.png`)
- Viewer (unseal host offer, post sealed answer, then receive PNG frames to disk):
  - Get the host compact envelope from the printed host output or from the JSON file in `./cabana-fixture`.
  - `cargo run --manifest-path apps/beach-cabana/Cargo.toml --features webrtc -- webrtc-viewer-run --session-id demo --passcode secret --host-envelope '<compact-string>' --fixture-url http://127.0.0.1:8081/signaling --recv-frames 60`
  - Saves into `temp/cabana-viewer-<ts>` by default (override with `--output-dir`).

Notes
- This demo uses full-ICE (non-trickle) with a short wait for gathering to complete before sealing SDP.
- The host polls the fixture directory to detect the viewer answer (since the fixture server is write-only). This keeps the demo self-contained.
- Script: `bash temp/cabana-phase3-demo.sh [--stream] [--codec h264|png] [--frames N] [--interval-ms MS] [--max-width W] [--play]`

Beach Road flow (recommended)
- Create or reuse a session (optional helper):
  - `beach-cabana road-create-session --road-url http://127.0.0.1:8080` (prints URL + join code)
  - `beach-cabana road-join-session --road-url http://127.0.0.1:8080 --session-id <ID>`
- Host: 
  - `beach-cabana webrtc-host-run --session-id <ID> --passcode <secret> --road-url http://127.0.0.1:8080 --from-id host --to-id viewer [--window-id display:<ID> --codec h264|gif --frames 60 --interval-ms 33 --max-width 1280]`
- Viewer:
  - `beach-cabana webrtc-viewer-run --session-id <ID> --passcode <secret> --road-url http://127.0.0.1:8080 --from-id viewer --to-id host --recv-frames 60`

Next steps
- CLI pairing for sealed offer/answer (host/viewer), with optional fixture POST to `beach-cabana fixture-serve` for manual two‑terminal workflows.
- Bind the secure transport to send encoded frames (start with GIF/H.264 keyframes) over the data channel.
- Viewer integration: beach-surfer renders the received stream, shows verification code, and surfaces basic controls.

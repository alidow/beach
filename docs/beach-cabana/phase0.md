# Beach Cabana – Phase 0 Research Package

Goal: Document feasibility, required APIs, and build boundaries for GUI screen sharing before touching the existing Beach terminal host.

## Capture API Matrix

| OS | Primary API | Capabilities | Notes |
| --- | --- | --- | --- |
| macOS 12.3+ | ScreenCaptureKit (AVFoundation) | Enumerate windows/displays, 60fps+, audio capture, hardware-friendly pixel buffers | Requires Screen Recording entitlement; ships as part of macOS; Objective-C/Swift bridge. |
| macOS <=12.2 fallback | CGWindowList + IOSurface + AVAssetWriter | Window list, manual compositor, lower FPS, no direct audio | Only use if ScreenCaptureKit unavailable; more CPU intensive. |
| Windows 10 2004+ | Windows Graphics Capture (WinRT) | Real-time window/monitor capture, Direct3D surface output, integrates with Media Foundation encoders | Needs app manifest capability; handles UWP-style permission toast. |
| Windows 8.1/7 fallback | DXGI Desktop Duplication | Monitor capture only, no window cropping; still useful for full desktop | No built-in permissions; manual filtering needed. |
| Linux (Wayland) | XDG Desktop Portal / PipeWire | Portal prompt enumerates monitors/windows, delivers DMA-BUF/SHM frames, supports FPS hints | Requires portal availability; user must approve each session. |
| Linux (X11) | XShm + XComposite (via FFmpeg/OBS patterns) | Window and screen capture with shared memory; no permissions | Works on legacy X11; needs manual damage tracking. |

## Hardware Encoding Options

| OS | Primary Hardware Encoder | API Surface | Notes |
| --- | --- | --- | --- |
| macOS | VideoToolbox H264/HEVC | VTCompressionSession | Works with CVPixelBuffer from ScreenCaptureKit; requires keyframe pacing logic. |
| Windows | Media Foundation + GPU MFT (NVENC/AMD/Intel Quick Sync) | IMFTransform pipeline | For fallback, use x264 via libx264 if hardware MFT unavailable. |
| Linux | VAAPI (Intel), NVENC (NVIDIA), AMF (AMD), PipeWire DMA-BUF | GStreamer/FFmpeg wrappers | Need capability detection per host; fallback to x264 with CPU limiters. |

Encoder abstraction proposal:
- Trait `VideoEncoder` with methods `configure`, `encode_frame`, `request_keyframe`, `flush`.
- beach-cabana plugs in platform-specific implementations; expose knobs for target bitrate, fps, resolution cap.

## Permission & UX Requirements

| OS | Permission Flow | UX Considerations |
| --- | --- | --- |
| macOS | First launch triggers system dialog; requires `NSMicrophoneUsageDescription` if audio later; must guide user to Security & Privacy if previously denied. | Show pre-permission explainer; detect `CMIOHardware` denial and prompt re-launch instructions. |
| Windows | Windows Graphics Capture triggers system toast on first use per app; app must include `graphicsCapture` capability. | Provide inline instructions if user dismissed toast; fallback to `OpenPicker` to re-request. |
| Linux Wayland | Portal displays chooser dialog (modal). Without portal, capture denied. | Detect portal unavailability early; instruct user to install `xdg-desktop-portal` backend; handle user cancel gracefully. |
| Linux X11 | No permission prompts. | Warn users about privacy implications; optionally support inclusion/exclusion list to avoid sensitive windows. |

## Window Identification Strategy

| OS | Identifier | CLI Flag Example | Notes |
| --- | --- | --- | --- |
| macOS | `CGWindowID` (uint32) + bundle_id/name metadata | `beach cabana --window-id 12345` | Need refresh to handle reused IDs; also expose `--display-id`. |
| Windows | HWND (hex) + title/process name | `beach cabana --window-id 0x00030ABC` | Provide helper `cabana list-windows` to reveal HWND + AppUserModelID. |
| Linux Wayland | Portal token (opaque) | `beach cabana --window-token abc123` | Tokens issued per portal session; CLI flow primarily uses interactive picker. |
| Linux X11 | Window XID (hex) | `beach cabana --window-id 0x4A00007` | `xwininfo`-style enumeration via Xlib in helper utility. |

Expose a unified struct:
```rust
pub struct CaptureTarget {
    pub kind: TargetKind, // Window | Display | Region
    pub identifier: String, // OS-specific token/Stringified ID
    pub human_label: String, // "Chrome — PRD Dashboard"
}
```

## Prototype Spikes

1. **Window Enumeration CLI**
   - macOS: Swift command using ScreenCaptureKit `SCShareableContent`.
   - Windows: Rust + WinRT crate to list `GraphicsCaptureItem::try_create_from_window`.
   - Linux: Rust crate leveraging `ashpd` (portal) and fallback X11 bindings.
   - Output JSON list consumed by CLI/TUI prototypes.

2. **Frame Capture Preview**
   - Minimal Rust binary per OS capturing a chosen target and writing frames to a temporary file or simple SDL2 window.
   - Validate frame size, latency, and CPU load; record metrics for plan.

3. **Encoder Round-Trip**
   - Feed captured frames into chosen hardware encoder; write `.webm/.mp4` sample; confirm decode in VLC.
   - Collect bitrate vs. quality data for 1080p60 and 1440p30.

Artifacts from each spike should be committed under `research/beach-cabana/` (private) and referenced, but not yet wired into Beach apps.

## Zero-Trust Alignment (Secure Signaling & Transport)

- Cabana adopts the workflow defined in `docs/secure-webrtc/secure-shared-secret-webrtc-plan.md`.
- Each unique share link + passcode feeds an Argon2id stretch to produce a pre-shared key; Cabana must treat the passcode as mandatory and never send it to `beach-road`.
- Signaling blobs (offer/answer/ICE) are sealed client-side using AES-256-GCM or ChaCha20-Poly1305 with keys derived via HKDF from the stretched secret and handshake id.
- After data channel establishment, Cabana peers run a Noise `XXpsk2` handshake, binding to DTLS exporter/SDP fingerprints, and derive per-direction AEAD keys for video/control frames.
- Host and viewer display a short authentication string so participants confirm they share the same keys before exposing the desktop.
- Spike deliverable: prototype encrypt/decrypt helpers plus Noise handshake flow within Cabana’s research binaries, validated against tampered signaling transcripts.

## Phase 1 Scaffold Snapshot

- `apps/beach-cabana/` crate created as a standalone binary (`beach-cabana`) excluded from the main Beach workspace to avoid touching terminal code paths yet.
- CLI commands in place: `list-windows` (macOS returns live window + display metadata via Core Graphics; other OSs show placeholders), `preview` (macOS captures the selected window/display to a PNG in the temp directory), `stream` (loops captures into a temp directory to simulate continuous frames ahead of ScreenCaptureKit), `encode` (records a short session into an animated GIF as a first encoder integration), `start` (derives session keys, seals either random probes or real payloads via `--payload-file`, optionally POSTing to a local fixture), dev utilities `seal-probe` / `open-probe`, and `fixture-serve` (tiny_http-based beach-road stub that persists sealed envelopes to disk).
- Capture module scaffolded (`apps/beach-cabana/src/capture/`) with a shared `FrameProducer` trait, Core Graphics fallback producer, and ScreenCaptureKit stubs behind the `cabana_sck` feature flag.
- Security helpers implemented: Argon2id stretch + HKDF derivation, ChaCha20-Poly1305 sealing/unsealing, compact envelope format with base64 encoding, and handshake-id utilities.
- Build hint: `cargo check --manifest-path apps/beach-cabana/Cargo.toml` validates without requiring other Beach crates.
- Next up: integrate ScreenCaptureKit for continuous capture (current preview is a single-frame snapshot). See `docs/beach-cabana/screencapturekit-spike.md` for the detailed approach. After SCKit lands, wire sealed envelopes into a minimal WebRTC handshake harness so live frames exercise the zero-trust path.

## Licensing / Boundary Notes

- Open-source release (terminal-only) will include crates: `beach-core`, `beach`, `beach-web`, protocol definitions, and shared CLI scaffolding.
- Proprietary modules: `apps/beach-cabana`, platform-specific capture adapters (`macos`, `windows`, `linux` subcrates), hardware encoder wrappers, picker UI assets.
- Build Features:
  - `--features terminal` (default OSS build) compiles without beach-cabana.
  - `--features gui` enables cabana-specific crates and binaries.
- Continuous integration should enforce OSS build path by running `cargo check --features terminal --no-default-features`.
- Distribution pipeline for proprietary artifacts lives in a separate private repo or registry; ensure licensing headers reflect closed-source status.

## Open Questions & Next Steps

- Confirm minimum OS versions we are willing to support (e.g., drop pre-Windows 10 or macOS < 12.3?).
- Decide whether audio capture is MVP or a later phase; affects permissions and encoder muxing.
- Evaluate whether Beach desktop app remains Tauri or migrates to native shells for better picker integration.
- Phase 1 kickoff requires sign-off on trait boundaries and crate split outlined above.

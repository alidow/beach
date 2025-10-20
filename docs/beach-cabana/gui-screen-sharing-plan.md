# Beach Cabana Screen Sharing – Phased Plan

Goals
- Extend Beach beyond terminal streaming to support window/desktop capture with WebRTC delivery on macOS, Windows, and Linux.
- Reuse the existing Beach session, signaling, and auth flows so GUI sharing feels consistent with terminal sharing.
- Ship a Zoom-style picker in the Beach desktop app and a TUI picker/flag-driven flow in the CLI.
- Preserve the ability to open-source the terminal stack while keeping GUI capture code proprietary.

Non-Goals (Initial)
- Native viewer apps for desktop/mobile (web viewer only for now).
- Remote control/input for GUI sessions (view-only).
- Capturing protected content or DRM windows.

Feasibility Snapshot
- **Capture APIs exist and are mature:** ScreenCaptureKit (macOS), Windows Graphics Capture (Win32/WinRT), and PipeWire/GBM (Wayland) or X11 for Linux. They all support window enumeration and high-FPS capture; numerous OSS projects (OBS, Chromium) rely on them.
- **WebRTC video fits the existing transport:** We already use WebRTC for terminal data; adding a video track piggybacks on the same signaling handshake with minimal protocol changes.
- **Packaging to keep GUI proprietary is viable:** We can publish the shared crates (protocol, signaling) while keeping capture adapters and UI entry points in a separate closed-source workspace member.

Key Risks & Considerations
- **OS permissions & entitlements:** macOS screen recording permission, Windows UAC prompts, Wayland portal dialogs. We must surface clear UX flows and fallbacks.
- **Performance & resource usage:** 60fps 4K streams stress CPU/GPU; need hardware encoder access (VideoToolbox, Media Foundation, VAAPI) or downscale/limit FPS.
- **Window enumeration parity:** Different OSes expose different identifiers (HWND, CGWindowID, PipeWire Node). We must abstract identifiers yet allow advanced users to target a specific window via CLI.
- **Open-source boundary:** Clear separation between reusable crates and proprietary capture/selection modules; feature flags and build gating to ensure open-source artifacts compile without GUI code.
- **Security/privacy:** Prevent accidental capture of sensitive windows; require explicit user confirmation and expose “pause/stop sharing” affordances.

Architecture Overview
- **Shared Core (open-source):** Protocol, signaling, session orchestration, and WebRTC negotiation remain in shared crates (beach-core). Introduce abstractions for “media producers” that GUI and terminal hosts implement once Cabana is production-ready.
- **Closed-source beach-cabana module:** Implements capture adapters (`macOS::ScreenCaptureKit`, `Windows::GraphicsCapture`, `Linux::PipeWire`), window enumeration, hardware encoder integration, zero-trust signaling, and UI flows while living in a private workspace member (`apps/beach-cabana`). This module starts life standalone, without requiring modifications to existing Beach binaries.
- **Picker UI:** React/SwiftUI/Tauri-based selection modal for the desktop app; TUI picker for CLI via `crossterm`/`ratatui`. Both feed a normalized `CaptureTarget` back into the host module.
- **Web Viewer (open-source):** Beach-web adds a video element fed by WebRTC; retains terminal viewer for existing sessions. Long-term it can choose layout based on session type.
- **Zero-trust security:** Cabana adopts the sealed-signaling + Noise transport design from `docs/secure-webrtc/secure-shared-secret-webrtc-plan.md` so a compromised `beach-road` cannot read window content. Unique links and passcodes continue to derive per-session secrets that wrap both signaling and media frames.
- **Feature Gating:** Workspace layout ensures open-source release omits GUI host crate; binaries link conditionally based on build features. Integration with Beach (terminal host) happens only after Cabana’s standalone milestone.

Phased Delivery

**Phase 0 — Research & Spikes (beach-cabana)**
- Deliverables: matrix of OS capture APIs, hardware encoding options, permission requirements, and window ID schemes; prototype enumerating windows per OS; doc outlining licensing/open-source boundary.
- Success: Clear go/no-go on capture feasibility per OS; draft module boundaries approved.

- **Phase 1 — Cabana Standalone Foundations (Completed, 2025-02-XX)**
- Deliverables: Private `apps/beach-cabana` workspace with standalone CLI; capture abstraction (`capture/` module) with CoreGraphics producer + ScreenCaptureKit hooks; zero-trust signaling helpers; fixture tooling; documentation updates.
- Success: `beach-cabana` supports window enumeration, preview snapshots, scripted streaming, secure session bootstrap (`start`), and sealed payload fixture workflows without touching `apps/beach-*`.
- Notes: ScreenCaptureKit integration is feature-gated via `--features cabana_sck` and spec'd for Phase 2; current builds default to the CoreGraphics producer for macOS.

**Phase 2 — Capture & Encoding Adapters (Completed, 2025-02-XX)**
- Deliverables: Capture abstraction expanded with resize/fps controls, `GifVideoEncoder` software fallback, CLI `encode` command producing animated GIFs, initial ScreenCaptureKit hook points behind the `cabana_sck` feature gate.
- Success: Cabana can capture and downscale frames via the new producer API and encode them into a shareable artifact without relying on Beach core binaries.
- Notes: Hardware VideoToolbox/Media Foundation integrations remain follow-ups (Phase 2.1) once the ScreenCaptureKit bridge lands; the encoder trait is ready to accept those adapters.

**Phase 3 — Zero-Trust Signaling & Media Pipeline**
- Deliverables: Implement sealed signaling (Phase 1 spec in `docs/secure-webrtc/secure-shared-secret-webrtc-plan.md`) inside Cabana; run Noise `XXpsk2` handshake over data channel; wrap outgoing media frames (video + control) in AEAD using keys derived from the unique link + passcode.
- Success: Cabana peers exchange unique link/passcode, establish WebRTC video channel via `beach-road` while keeping signaling opaque; tampering at the relay fails verification.
- Progress:
  - Noise stack implemented with `NoiseController`, transport AEAD, replay protection, and verification code (`apps/beach-cabana/host/src/noise.rs`).
  - Channel-agnostic `NoiseDriver` plus a `webrtc` feature that adapts the driver to real data channels (`apps/beach-cabana/host/src/webrtc.rs`).
  - New local E2E demo (behind `--features webrtc`): `beach-cabana webrtc-local --session-id ... --passcode ...` spins up two in-process peers, opens a real WebRTC data channel, performs the Noise handshake, returns a verification code, and exchanges encrypted media messages. This provides an agent-friendly way to exercise Phase 3 without external signaling.
  - Sealed signaling helpers are already integrated (Phase 1 `start` path) and remain the basis for offer/answer sealing. Next step is wiring sealed offer/answer to a fixture exchange and landing a minimal “host-offer/viewer-answer” CLI pairing.

Beach Road integration (done)
- Host/Viewer now post/get sealed SDP via Beach Road endpoints. CLI helpers exist to create/join sessions (`road-create-session`, `road-join-session`). Host and viewer print a passcode fingerprint and the Noise verification code so users can confirm zero-trust linkage.

CLI (feature `webrtc`) additions
- `webrtc-local` — local in-process demo (no external signaling)
- `webrtc-host-run` — host generates sealed offer (optionally POSTs to fixture), polls fixture dir for viewer answer, completes Noise over data channel, prints verification code
- `webrtc-viewer-answer` — viewer unseals host offer, generates sealed answer, optionally POSTs to fixture

Next Phase 3 steps:
- Add sealed offer/answer subcommands and fixture polling glue for a manual two-terminal flow.
- Hook the secure transport to stream encoded frames over data channel (start with GIF/H.264 keyframe pacing on macOS).

**Phase 4 — Selection UX (Desktop App & CLI)**
- Deliverables: Zoom-style picker modal in Cabana desktop app prototype; CLI TUI menu with arrow/enter flow plus `--window-id` flag; permission prompts surfaced with guidance; verification string surfaced post-handshake so users confirm zero-trust link.
- Status (2025-10-18):
  - CLI TUI picker implemented with Displays/Windows tabs, type-to-filter, refresh, and selection. Integrated into `webrtc-host-run`: if `--window-id` is absent and streaming requested, the picker launches.
  - Post-handshake verification gate implemented: host bootstrap completes Noise + prints the 6-digit verification code; CLI prompts for confirmation before streaming frames.
  - macOS permission preflight in host platform module; CLI surfaces guidance and calls request API before starting capture.
  - Desktop picker scaffolded as a minimal native app under `apps/beach-cabana/native-apps/desktop` (terminal UI placeholder). Next step: replace with Tauri UI.
- Success: User selects target via app or CLI; correct target streams securely; zero‑trust verification confirmed before streaming. Desktop UI to be upgraded next.

Suggested implementation notes (handoff-ready)
- CLI TUI picker (ratatui/crossterm):
  - Source window list from existing `platform::enumerate_windows()` and refresh on user request.
  - Show two tabs: Displays and Windows; capture `CGWindowID`/`display:N` selection; produce a `CaptureTarget`.
  - Integrate with `webrtc-host-run` by passing `--window-id` when the user confirms.
- Desktop picker (Tauri/SwiftUI placeholder):
  - Wrap the same enumeration calls via a small FFI layer; focus on macOS first.
  - Show permissions guidance if Screen Recording is not granted; re-run permissions check after user interaction.
- Post-handshake UX:
  - Show the short Noise verification code in host and viewer UI; require user confirmation before starting capture stream.
  - Surface pause/stop controls and an obvious “Stop Sharing” affordance.

**Phase 5 — Web Viewer Experience**
- Deliverables: Update beach-surfer to detect Cabana (GUI) session type, render video player with basic controls (pause, fit-to-window, resolution info), display verification hash; responsive layout coexisting with terminal viewer; metrics overlay for debugging.
- Success: Viewers join via browser using unique link + passcode and watch at >=30fps with acceptable latency; verification hash matches host’s display.

**Phase 6 — Integration with Beach Core**
- Deliverables: Refactor `beach-core` traits to support both terminal and GUI media producers; feature gating to keep OSS builds terminal-only; CLI/app entry points delegate to Cabana modules when `gui` feature enabled.
- Success: Terminal sharing remains unaffected; enabling Cabana adds GUI option without regressions; open-source boundary maintained.

**Phase 7 — Hardening & Operational Readiness**
- Deliverables: Automated QA scripts for multi-monitor setups; stress/perf benchmarks; observability (frame rate, encoder health, permission failures, key rotations); pause/stop sharing controls; documentation for operators including zero-trust guidance.
- Success: Beta-ready GUI sharing across supported OSes with documented limitations; bugs tracked for edge cases (Wayland portal quirks, HDR, high-DPI); zero-trust requirements verified end-to-end.

Future (Post-MVP)
- Native viewers (desktop/mobile) consuming the same WebRTC signaling.
- Remote control/input and multi-user collaborative controls.
- Recording/archiving pipelines with consent workflows.
- Enterprise policy controls (allowed window lists, compliance logging).

# Beach Cabana Screen Sharing – Phased Plan

Goals
- Extend Beach beyond terminal streaming to support window/desktop capture with WebRTC delivery on macOS, Windows, and Linux.
- Reuse the existing Beach session, signaling, and auth flows so GUI sharing feels consistent with terminal sharing.
- Ship a Zoom-style picker in the Beach desktop app and a TUI picker/flag-driven flow in the CLI.
- Preserve the ability to open-source the terminal stack while keeping GUI capture code proprietary.

Progress Snapshot (2025-10-18)
- CLI webrtc flow now uses `host_bootstrap` + verification gate; capture only starts after the user confirms the Noise code matches.
- TUI picker includes Displays/Windows tabs, live filter, refresh, and an ASCII preview generated from `platform::preview_window`.
- macOS permission preflight wrappers live in `host::platform::macos::permissions`; the CLI warns and triggers the OS prompt before streaming.
- Desktop picker workspace (`apps/beach-cabana/native-apps/desktop`) added with a terminal placeholder to be replaced by a Tauri modal.

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

**Phase 3 — Zero-Trust Signaling & Media Pipeline (Completed, 2025-10-20)**
- Deliverables: Implement sealed signaling (Phase 1 spec in `docs/secure-webrtc/secure-shared-secret-webrtc-plan.md`) inside Cabana; run Noise `XXpsk2` handshake over data channel; wrap outgoing media frames (video + control) in AEAD using keys derived from the unique link + passcode.
- Success: Cabana peers exchange unique link/passcode, establish WebRTC video channel via `beach-road` while keeping signaling opaque; tampering at the relay fails verification. Host and viewer now require a matching verification string before media flows.
- Highlights:
  - Noise stack + data-channel driver power the secure transport (`apps/beach-cabana/host/src/noise.rs`, `webrtc.rs`).
  - `host_bootstrap` can post sealed offers either to Beach Road or a local fixture and poll fixture directories/HTTP endpoints for the viewer reply, enabling fully offline rehearsals.
  - `viewer_run` publishes sealed answers back to the selected relay and enforces a confirmation callback so operators must acknowledge the Noise verification code.
  - CLI ergonomics: `webrtc-host-run` forwards `--fixture-url/--fixture-dir`, pauses for a picker when no window id is provided, and blocks streaming until the operator approves the verification string; `webrtc-viewer-run` now prompts for the matching code before frames are written.
  - Local WebRTC demo (`webrtc-local`) remains available as a regression harness.

- **Phase 4 — Selection UX (Desktop App & CLI)**
- Deliverables: Zoom-style picker modal in Cabana desktop app prototype; CLI TUI menu with arrow/enter flow plus `--window-id` flag; permission prompts surfaced with guidance; verification string surfaced post-handshake so users confirm zero-trust link.
- Status (2025-10-19):
  - Completed. CLI commands auto-launch the picker when `--window-id` is omitted, request platform-specific screen-share permissions up front, and block streaming until operators accept the Noise verification code.
  - Viewer flows now display the verification string and can abort gracefully (`NoiseDriverError::UserAborted`) when the code does not match, letting higher-level tooling branch cleanly.
  - Desktop picker ships as an `eframe` modal with tabs, search, live previews, confirm/cancel controls, one-click copy/open preview actions, and banners for macOS, Windows, and Linux portal flows. Confirmed selections are published over the new `beach_cabana_host::desktop` relay so the host shell can subscribe. CLI consumers can set `CABANA_PICKER_RELAY=1` or `CABANA_PICKER_WAIT_MS=…` to wait for the desktop picker before falling back to the TUI.
- Next actions: hook the desktop host shell into the relay when the GUI capture adapters land, and replace the placeholder Windows/Linux enumeration with actual window/display discovery.

Suggested implementation notes (handoff-ready)
- CLI TUI picker (ratatui/crossterm):
  - Source window list from `platform::enumerate_windows()` and refresh on demand.
  - Provide Displays/Windows tabs, filtering, and emit a `CaptureTarget` used by `webrtc-host-run` when `--window-id` is omitted.
- Desktop picker (eframe prototype):
  - Uses the shared enumeration API, renders live previews, surfaces permission guidance, exposes quick actions (copy identifier, open preview), and publishes selections through the host relay. Replace the placeholder enumeration/stub previews once platform adapters ship.
- Post-handshake UX:
  - Host and viewer now display the verification code and require confirmation pre-stream.
  - Still pending: dedicated pause/stop controls and a richer “sharing” HUD once capture begins.

**Phase 5 — Web Viewer Experience (Completed, 2025-10-20)**
- Deliverables: Update beach-surfer to detect Cabana (GUI) session type, render video player with basic controls (pause, fit-to-window, resolution info), display verification hash; responsive layout coexisting with terminal viewer; metrics overlay for debugging.
- Highlights:
  - `BeachViewer` now layers a control HUD with pause/resume, fit mode cycling (contain/cover/actual pixels), and live stats toggles. PNG streams use the upgraded canvas renderer, while H.264 streams feed a `MediaSource` player with the same control surface.
  - Host-side H.264 output is packaged as fragmented MP4 via the new Rust `Fmp4Writer`, so `--codec h264` delivers browser-ready init segments + `moof/mdat` fragments without a transmux step.
  - Both media paths surface per-second stats (resolution, FPS, bitrate, total frames, buffer depth) and expose codec fingerprints alongside the Noise verification badge.
  - Viewer transports emit actionable errors (unsupported codec, append failures) and continue to honour the Noise confirmation gate introduced in Phase 4.
- Follow-ups: extend the stats overlay with historical timelines, add keyboard shortcuts for controls, and wire the metrics feed into observability once Phase 7 begins.

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

## Prioritized Next Steps (2025-10-21 Update)

1. **Picker parity sprint (Phase 4 polish)** *(macOS picker now renders tile grid with window titles + application labels; Windows/Linux adapters still pending)*  
   - Ship OS-native pickers wherever the platform provides them (macOS `SCContentSharingPicker`, Windows Graphics Capture UI, Wayland portal) and fall back to our custom gallery only when no native shell exists (see `docs/beach-cabana/macos-picker/plan.md`).  
   - Surface window titles, owning application bundle, and display identifiers inside both the native picker list and the preview tiles.  
   - Replace the single-preview layout with a Zoom-style grid that renders all windows/displays at once, adds live refresh, and highlights the currently focused option.  
   - Keep the relay contract unchanged so the CLI picker continues to receive selections with no additional wiring.  
   - Follow-up: hide the picker window when capturing display previews, move preview capture off the UI thread, and return occluded/background windows so macOS parity matches Zoom-like pickers. Track Zoom behaviour as the baseline UX whenever we design custom flows.

2. **Embed the host engine in the desktop app** *(desktop picker now boots Cabana host/WebRTC end-to-end with in-app session controls and verification prompts)*  
   - Link `beach_cabana_host` directly into `native-apps/desktop`, letting the native picker trigger capture, codec selection, and WebRTC bootstrapping without the CLI helper.  
   - Maintain a thin CLI front-end that calls the same host APIs, preserving scriptability while proving that the GUI path can stand alone.  
   - Audit permissions and lifecycle so ScreenCaptureKit (macOS), Noise verification, and signaling prompts surface in-window.

3. **Native-to-web viewing path**  
   - Use the embedded host to originate a full Cabana WebRTC session, then validate that Beach Surfer can attach and render the stream in real time from the fragmented-MP4 pipeline.  
   - Add automated smoke flows that launch the desktop host, start a session, and drive a headless Surfer instance to confirm end-to-end playback.

4. **Beach Surfer component architecture**  
   - Introduce a `BeachSessionView` React component that selects between `TerminalViewer` and the new `CabanaViewer` based on session kind, sharing data-fetching hooks and zero-trust validation.  
   - Keep the terminal renderer untouched while building the Cabana viewer on top of the existing `MediaVideo/MediaCanvas` work, exposing a clean props surface for metrics, controls, and verification state.  
   - Document usage and ensure the component API is ergonomic enough to embed in future shells.

5. **Stretch goal: remote input preview**  
   - Prototype a secure input channel (mouse/keyboard) tunneled through the existing Noise transport, gated behind an explicit host opt-in.  
   - Start with basic pointer move/click events and keyboard strokes, replayed locally on macOS via Accessibility APIs.  
   - Treat this as experimental until hardened telemetry and permission UX are in place.

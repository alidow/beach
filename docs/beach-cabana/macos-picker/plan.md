# macOS Native Picker & Session Builder – Implementation Plan

Last updated: 2025-11-02  
Owner: Cabana team (macOS lead)  
Goal window: Phase 4.1 (picker parity) → Phase 4.2 (session creation UX)

## Status snapshot (2025-11-02)

- ✅ `cabana-macos-picker` crate now exposes a real macOS bridge: the Swift/ObjC layer wraps `SCContentSharingPicker`, serializes `SCContentFilter`, and streams selection events into Rust. A mock backend still ships for CI/non-macOS.
- ✅ Desktop binary links the crate under feature flags; a background listener can already emit selection events for manual vetting.
- ⚠️ The legacy egui gallery remains on-screen. We still need to replace it with the native picker-driven UX (tiles, session sheet, auth).
- ⚠️ Host runtime, Clerk auth, and Beach Road/Private Beach wiring continue to expect the old window-id contract.

## Path to Beach-ready UX

To achieve the product milestone (“launch, pick any non-minimized screen, sign in, publish publicly or to a private beach, and view in Beach Surfer”), we will execute the remaining work in the phases below. Each phase calls out blockers toward the three headline behaviours:

1. **Tile view of all windows/displays (native picker parity).**
2. **Clerk-powered authentication + private beach selection.**
3. **End-to-end streaming: Cabana ▶ Beach Road/Private Beach ▶ Beach Surfer React components.**

## Why we are doing this

Our egui-based picker struggles to match Zoom-quality UX:

- heavy CGWindow snapshots block the UI; labels aren’t readable; only on-screen windows are listed; screen previews mirror the picker window.
- ScreenCaptureKit already ships a native picker (`SCContentSharingPicker`) that Apple apps and Zoom leverage for smooth enumeration and live preview.
- Hosts need a single workflow that (a) selects the capture source, (b) establishes a Beach session, and (c) returns a public link or publishes to a private beach once the user authenticates.

This plan replaces the custom picker on macOS with a native picker + session setup wizard and wires outputs into Beach Road / Private Beach.

## Desired UX (zoom parity baseline)

1. Launch Cabana desktop app.
2. Native sheet opens listing all displays/windows (including hidden/minimized) with live preview.
3. User picks a tile:
   - If logged out → prompt for public Beach session (link + passcode).
   - If logged in via Clerk → option to publish to one of their private beaches or create a new public share.
4. App displays verification code, session link, and quick actions (copy link, open in browser).
5. Session starts streaming immediately; the picker sheet can be reopened to switch targets.

## Non goals (for this phase)

- Windows/Linux pickers (keep existing gallery until platform-native equivalents are identified).
- Remote input (mouse/keyboard).  
- Viewer UI changes beyond what is already planned in Beach Surfer.  
- Advanced session management (multi-peer, scheduling).

## Architecture overview

### Components

| Component | Role |
| --- | --- |
| `macos-picker-bridge` (new Swift library) | Wraps `SCContentSharingPicker` and ScreenCaptureKit filters; exposes a C ABI for Rust. |
| `cabana_macos_picker` (new Rust crate) | FFI bindings + safe wrappers for the bridge; provides async API for listing capture targets and receiving selected `SCStreamConfiguration`. |
| Desktop shell (`native-apps/desktop`) | Replaces egui picker with thin Swift UI: triggers native picker, renders session setup flow, and passes selection to host runtime. |
| Host runtime (`beach_cabana_host`) | Accepts ScreenCaptureKit filters directly (where available) or maps them to existing identifiers for fallback capture. |
| Session service integration | After selection, the app creates or updates a session via Beach Road. If the user is authenticated (Clerk), it offers private beach publishing. |

### Data flow

1. UI requests picker launch from `macos-picker-bridge`.
2. Native picker returns `SCContentFilter` + metadata (window title, bundle id, preview).
3. Rust wrapper converts filter into a serialized descriptor (store identifier + filter settings).
4. App displays session configuration sheet (link/passcode).  
5. On confirm, desktop shell:
   - authenticates via Clerk (if user chooses private beach)  
   - calls Beach Road API to create session (public) or Private Beach API to publish
   - launches host stream using the serialized filter (ScreenCaptureKit first, fallback to CGWindow image capture if filter unsupported).
6. App shows status card with link, passcode, verification code, and controls to stop/share.

## Detailed implementation plan

### Phase 0 – Research & scaffolding (1–2 days)

1. **Evaluate SCContentSharingPicker APIs**
   - Document available delegates (`SCContentSharingPickerDelegate`) and selection structures (e.g., `SCShareableContent`, `SCContentFilter`).
   - Prototype in standalone Swift command-line or SwiftUI app to understand lifecycle (one-time vs persistent picker).  
   - Measure support for multiple streams and background windows (set `onScreenWindowsOnly = false`).

2. **Select bridging approach**
   - Option A: create a Swift package and call it via `cxx` crate (Rust↔︎C++).  
   - Option B: embed a small Objective-C wrapper compiled as a static library and expose C functions.  
   - Choose whichever gives stable ABI without requiring `cxx` in the main crate (lean toward ObjC bridging because we only need a few callbacks).

3. **Define data model**
   - `PickerResult`: `{ id: String, label: String, bundle_id: Option<String>, filter_blob: Vec<u8> }`.  
   - `filter_blob` stores either serialized SCContentFilter or fallback window/display identifier.  
   - Provide JSON serialization for use by the desktop shell and host runtime.

Deliverables:
- `docs/beach-cabana/macos-picker/spike-notes.md` capturing API findings and recommended bridge approach.

### Phase 1 – Native picker bridge (Status: **Complete**)

**What’s done**
- Objective-C bridge compiled via `build.rs`, linking ScreenCaptureKit/AppKit and exporting a C ABI.
- Rust `PickerHandle` provides `launch`, `listen`, `stop` with async stream of `PickerEvent::{Selection,Cancelled,Error}`.
- Mock feature gate retains CI coverage.

**Remaining acceptance items**
- [ ] Harden error reporting/telemetry for picker availability (wire into desktop logger once UI lands).

### Phase 2 – Picker-driven desktop UX (**In progress**)

**Goals**
1. Display picker-provided tiles (including hidden/minimized windows and displays) in the Cabana shell.
2. Persist the latest `PickerResult` (filter blob + metadata) for reuse (desktop relay + CLI).
3. Provide a Swift/egui session sheet that can:
   - Trigger native picker re-open.
   - Surface preview thumbnail, session type (Public / Private), and quick actions (copy link, open Surfer).

**Tasks**
- [ ] Replace egui gallery with a view that consumes `PickerHandle::listen()` and renders tiles (no SwiftUI yet—initially egui backed by picker metadata).
- [ ] Serialize `SCContentFilter` + optional `SCStreamConfiguration` into a `ScreenCaptureDescriptor` struct and attach it to `SelectionEvent`.
- [ ] Update `beach_cabana_host::desktop::publish_selection` callers to forward descriptors so CLI and desktop remain in sync.
- [ ] Build minimal “session sheet” inside the desktop shell (can remain egui while we iterate) showing:
  - Current selection metadata (title, bundle id, secure badge if private beach).
  - Session inputs (passcode, session id).
  - Buttons for `Start public share` / `Attach to private beach` (stubbed until Phase 3).
- [ ] Wire telemetry hooks (`picker_open`, `picker_selection`) once tiles render.

**Exit criteria**
- Launching the macOS binary shows all non-minimized screens/windows as tiles populated from the native picker (goal #1).
- Selecting a tile updates desktop state + publishes to the relay (e.g., CLI sees the same descriptor).
- Manual smoke test logs streamed picker events in the new UI.

### Phase 3 – Auth + session orchestration (**Not started**)

Focus: deliver goals #2 and #3 (Clerk auth, Beach session wiring, Beach Surfer playback).

**Auth & session creation**
- [ ] Integrate Clerk desktop sign-in (reuse `beach login` flow; share tokens via secure Keychain storage).
- [ ] Fetch private beach inventory post-login; expose picker allowing users to choose target private beach or fallback to “Public Share”.
- [ ] Public path: call Beach Road to allocate session id + passcode, auto-fill sheet, generate copy / open actions.
- [ ] Private path: PATCH/POST to Private Beach API attaching Cabana session metadata (viewer worker + credentials) to the selected beach.
- [ ] Surface verification code, session link, and clipboard/share buttons in-session sheet.

**Streaming pipeline**
- [ ] Extend host runtime (`host_bootstrap`, ScreenCaptureKit adapter) to consume serialized descriptors; implement fallback to CGWindow capture when building older binaries.
- [ ] Update `beach_cabana_host::webrtc::host_stream` to detect descriptor type and spin up ScreenCaptureKit streaming with live updates.
- [ ] Ensure CLI flows remain functional by accepting descriptors (no regression for terminal sessions).

**Beach Surfer React components**
- [ ] Introduce reusable Surfer player components capable of rendering PNG/H.264 Cabana sessions (public + private) with minimal props (session id/passcode OR private beach id).
- [ ] Update Surfer pages to consume the new components and handle real-time updates (Noise handshake, viewer metrics).
- [ ] Document ergonomic usage patterns for other Beach surfaces.

**Exit criteria**
- User can sign in via Clerk, select either public session or private beach target, and start streaming.
- Beach Surfer shows the live feed when given session id/passcode or when visiting the chosen private beach.
- QA script covers both flows end-to-end.

### Phase 4 – Polish & parity (2–3 days)

1. UX improvements:
   - Allow reopening picker to switch windows without restarting host stream.  
   - Display verification code + secure badge while streaming.  
   - Add error handling (picker cancellation, API failures).

2. Accessibility & localization:
   - Ensure labels use dynamic type and support VoiceOver (native picker already does; confirm for session sheet).  
   - Provide alt text for thumbnails.

3. Telemetry:
   - Emit timing metrics (`picker_open`, `selection_received`, `session_created`, `stream_started`).  
   - Log selection counts by kind (display/window) and API failure rates.

4. Documentation & handoff:
   - Update `docs/beach-cabana/gui-screen-sharing-plan.md` with outcomes.  
   - Add usage guide to `docs/beach-cabana/macos-picker/usage.md`.  
   - Write regression checklist for QA (permissions, login, link copy).

Deliverables:
- Fully operational macOS native picker & session builder shipping in Cabana desktop app (tiles + session sheet + streaming).  
- QA checklist + telemetry dashboards instruments defined.  
- Updated docs referencing new flow.

## Dependencies & open questions

- **Clerk integration:** confirm desktop app already has a sign-in flow; if not, add one (webview or native). Tokens must be reusable by CLI (`beach login`).
- **Distribution:** Swift bridge needs to compile for both Intel & Apple Silicon; ensure `swiftc` invocation produces universal binary.
- **Testing on CI:** we may need to stub the picker by running tests in headless mode (no GUI). Provide environment variable `CABANA_NATIVE_PICKER=mock` to bypass UI during CI.
- **Private beach APIs:** verify attach/update endpoints support ScreenCaptureKit descriptors and multi-session state before wiring the UI.

## Risks & mitigations

- **App not running on main thread:** ScreenCaptureKit requires the picker to be invoked on the main queue. Mitigate by ensuring desktop app enters `NSApplicationMain` before starting Rust runtime.  
- **Permissions:** if Screen Recording permission is denied, picker will still appear but no previews. Add pre-flight check and clear guidance to enable in System Settings.  
- **Auth failure:** ensure public session path does not depend on Clerk; gracefully degrade if sign-in fails.

## Handoff checklist

- [ ] Swift bridge module committed with documentation.  
- [ ] Rust wrapper crate exposing typed API + tests.  
- [ ] Desktop app updated to use native picker with session sheet.  
- [ ] Host runtime updated to accept SCK descriptors.  
- [ ] Clerk integration implemented and documented.  
- [ ] Manual test instructions for QA.  
- [ ] Telemetry events defined and wired.

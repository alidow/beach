# macOS Native Picker & Session Builder – Implementation Plan

Last updated: 2025-10-21  
Owner: Cabana team (macOS lead)  
Goal window: Phase 4.1 (picker parity) → Phase 4.2 (session creation UX)

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

### Phase 1 – Native picker bridge (3–4 days)

1. Create `apps/beach-cabana/native-apps/desktop/bridge/macos_picker`:
   - `PickerBridge.swift` containing:
     - class `CabanaPickerController` implementing `SCContentSharingPickerDelegate`
     - bridging functions `cabana_picker_start`, `cabana_picker_stop`, `cabana_picker_current_selection`.
   - Use `@MainActor` and `DispatchQueue.main.async` to ensure UI operations happen on the main run loop.

2. Add new Rust crate `crates/cabana-macos-picker`:
   - Build script to compile the Swift sources via `swiftc` into a static library.  
   - `ffi.rs` with extern functions and conversion to Rust types.  
   - `PickerHandle` struct exposing:
     ```rust
     pub struct PickerHandle { /* ... */ }
     impl PickerHandle {
         pub fn launch(&self) -> Result<PickerResult>;
         pub fn listen(&self) -> impl Stream<Item = PickerResult>;
         pub fn stop(&self);
     }
     ```

3. Integrate bridging crate into `apps/beach-cabana/native-apps/desktop`:
   - On macOS, instantiate `PickerHandle` at startup; call `.launch()` to show the native picker.  
   - Replace egui gallery with new Swift sheet (the existing window can minimize/hide while the picker is shown).

4. Provide a fallback path for unit tests (mock results) to keep CI green on non-mac platforms.

Deliverables:
- Swift bridge compiling in the workspace (`cargo check --target x86_64-apple-darwin`).  
- Rust wrapper crate with simple demo (log selected identifier).  
- Basic manual test demonstrating the native picker returning selections.

### Phase 2 – Session setup UX (3–4 days)

1. Replace egui-based control panel with a native session sheet (SwiftUI or egui with simpler layout):
   - Fields: session name, stream type (Public / Private), passcode (auto-generated or custom).  
   - Buttons: “Copy Link,” “Open Beach Surfer,” “Stop Sharing,” “Switch Source.”
   - Show live preview thumbnail from the picker (ScreenCaptureKit provides preview frames; otherwise snapshot once).

2. Integrate Clerk authentication:
   - Reuse existing Desktop authenticator or embed Clerk’s Swift SDK (if available).  
   - Persist auth token securely (Keychain).  
   - `if authenticated` → fetch list of private beaches (REST call) and allow selecting one from a picker; otherwise show sign-in CTA.

3. Implement session creation flow:
   - Public session: call Beach Road (POST `/sessions`) to create link + join code.  
   - Private session: call Private Beach API to create/update session with viewer credentials.  
   - Store session metadata in app state so the host runtime can use it during streaming.

4. Hook the host runtime:
   - Extend `beach_cabana_host::webrtc::host_bootstrap` to accept `ScreenCaptureKitDescriptor` instead of plain window IDs.  
   - If SCK capture fails (unsupported OS), fall back to CoreGraphics via identifier as today.

Deliverables:
- Native session sheet with complete flow from selection → session creation → streaming start.  
- Authentication envelope tied to session creation API calls.  
- Manual test: select window, create public session, copy link, open in Surfer (verify playback).  
- Private beach test: sign in with Clerk, publish to private beach, verify entry exists.

### Phase 3 – Polish & parity (2–3 days)

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
- Fully operational macOS native picker & session builder shipping in Cabana desktop app.  
- QA checklist + telemetry dashboards instruments defined.  
- Updated docs referencing new flow.

## Dependencies & open questions

- **Clerk integration:** confirm desktop app already has a sign-in flow; if not, add one (webview or native).  
- **Distribution:** Swift bridge needs to compile for both Intel & Apple Silicon; ensure `swiftc` invocation produces universal binary.  
- **Testing on CI:** we may need to stub the picker by running tests in headless mode (no GUI). Provide environment variable `CABANA_NATIVE_PICKER=mock` to bypass UI during CI.

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

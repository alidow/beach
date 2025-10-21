# macOS Native Picker & Session Builder – Implementation Plan

Last updated: 2025-11-04  
Owner: Cabana team (macOS lead)  
Goal window: Phase 4.1 (picker parity) → Phase 4.2 (session creation UX)

## Status snapshot (2025-11-04)

- ✅ `cabana-macos-picker` crate now exposes a real macOS bridge: the Swift/ObjC layer wraps `SCContentSharingPicker`, serializes `SCContentFilter`, and streams selection events into Rust. A mock backend still ships for CI/non-macOS.
- ✅ Desktop binary now renders picker-fed tiles and a session sheet scaffold; selections emit the new descriptor payload + telemetry events.
- ⚠️ Session sheet flows are stubbed (Clerk/public/private Beach wiring still pending).
- ⚠️ Host runtime, Clerk auth, and Beach Road/Private Beach wiring continue to expect the old window-id contract.

## Path to Beach-ready UX

To achieve the product milestone (“launch, pick any non-minimized screen, sign in, publish publicly or to a private beach, and view in Beach Surfer”), we are splitting the remaining work into four parallel workstreams. Each workstream owns a vertical slice but collaborates on shared contracts (selection descriptor schema, API payloads, telemetry event names).

1. **Workstream A – Desktop Picker UX**: native tiles, session sheet, telemetry, user affordances.
2. **Workstream B – Host Runtime & Relay**: ScreenCaptureKit descriptor plumbing, capture fallback, CLI parity.
3. **Workstream C – Auth & Session Services**: Clerk integration, Beach Road/private beach APIs, attach flows.
4. **Workstream D – Beach Surfer Viewer**: reusable React components, public/private playback, ergonomics.

👉 Coordination checkpoints (weekly or when schemas change):
- Selection descriptor (`PickerResult` → `SelectionEvent` → host runtime) – A + B.
- Session creation payloads (Beach Road + Private Beach) – C + B (for streaming inputs) + D (viewer expectations).
- Telemetry + QA scripts – all workstreams.

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

### Workstream A – Desktop Picker UX (**In progress**)

**Goals**
1. Display picker-provided tiles (including hidden/minimized windows and displays) in the Cabana shell.
2. Persist the latest `PickerResult` (filter blob + metadata) for reuse (desktop relay + CLI).
3. Provide a Swift/egui session sheet that can trigger native picker re-open, surface preview, session type (Public / Private), and quick actions (copy link, open Surfer).

- [x] Replace egui gallery with a view that consumes `PickerHandle::listen()` and renders tiles (egui or SwiftUI shell TBD).
- [x] Define/UI-bind a `ScreenCaptureDescriptor` struct (mirrors Workstream B contract) and store it alongside metadata.
- [x] Publish selections (with descriptor) to relay/CLI (`SelectionEvent`).
- [x] Build minimal “session sheet” showing selection metadata, session inputs, and stub buttons for `Start public`, `Attach to private beach`.
- [x] Wire telemetry hooks (`picker_open`, `picker_selection`).

**Progress notes (2025-11-04)**
- Implemented egui tile grid populated from `PickerHandle` stream; tiles can be reselected without relaunching the picker.
- Introduced `ScreenCaptureDescriptor { target_id, filter_blob, stream_config_blob, metadata_json }` and persist it through `SelectionEvent` (`descriptor`, `label`, `application`).
- Telemetry currently logs `picker_open`/`picker_selection`/`picker_discovered` to stdout while we integrate with the shared metrics sink.
- Session sheet scaffolds public/private flows (inputs + stub buttons); Clerk/Beach API wiring remains under Workstream C.
- CLI now consumes the descriptor via `SelectionEvent.descriptor` for compatibility.

**Follow-ups**
- [ ] Align with Workstream B on ScreenCaptureDescriptor hydration (host fallback + tests).
- [ ] Replace stdout telemetry shim with production metrics pipeline hook once available.
- [ ] Integrate Clerk auth + Beach Road/Private Beach APIs (Workstream C dependency).

**Exit criteria**
- Launching the macOS binary shows all non-minimized screens/windows as tiles populated from the native picker (goal #1).
- Selecting a tile updates desktop state + publishes to the relay (e.g., CLI sees the same descriptor).
- Manual smoke test logs streamed picker events in the new UI.

### Workstream B – Host Runtime & Relay (**Not started**)

**Goals**
1. Teach `beach_cabana_host` to consume ScreenCaptureKit descriptors (with CG fallback) when launching streams.
2. Maintain CLI + desktop compatibility via extended `SelectionEvent` relay.
3. Provide test/mocks for CI (mock descriptor acceptance, ScreenCaptureKit gating).

**Tasks**
- [ ] Extend `SelectionEvent` to include serialized descriptor + metadata (agreement with Workstream A).
- [ ] Update `host_bootstrap` / `host_stream` to accept descriptors; branch to ScreenCaptureKit capture when available.
- [ ] Preserve legacy window-id path for older builds (feature toggle or env-based fallback).
- [ ] Add unit/integration tests for descriptor parsing; ensure mock mode works for CI.
- [ ] Emit telemetry around capture start/fallback.

**Progress – 2025-02-14**
- ✅ `SelectionEvent` now carries a `ScreenCaptureDescriptor` (target id, serialized ScreenCaptureKit filter/config blobs, optional metadata) and Cabana desktop publishes it on selection.
- ✅ `beach_cabana_host` decodes ScreenCaptureKit descriptors via `NSKeyedUnarchiver`; capture promotes SCK when hydration succeeds and logs `capture.backend` telemetry for both SCK and CoreGraphics backends.
- ✅ Automatic CoreGraphics fallback engages when descriptor decoding/enumeration fails or filters are unavailable; CLI consumers wrap legacy ids through `ScreenCaptureDescriptor::legacy`.
- ✅ Added identifier parsing coverage so `window:bundle:id` / `display:external:id` schemas function across SCK + CG paths; mock backend continues to work for CI.

**Exit criteria**
- Host can stream using ScreenCaptureKit descriptor provided by desktop UI.
- CLI workflows still function (no regressions).
- Tests cover descriptor parsing + fallback, CI green with mock path.

### Workstream C – Auth & Session Services (**In progress**)

Focus: deliver goals #2 and #3 (Clerk auth, Beach session wiring, Beach Surfer playback).

**Auth & session creation**
- [x] Integrate Clerk desktop sign-in (reuse `beach login` flow; share tokens via secure Keychain storage).
- [x] Fetch private beach inventory post-login; expose picker allowing users to choose target private beach or fallback to “Public Share”.
- [x] Public path: call Beach Road to allocate session id + passcode, auto-fill sheet, generate copy / open actions.
- [x] Private path: PATCH/POST to Private Beach API attaching Cabana session metadata (viewer worker + credentials) to the selected beach.
- [x] Surface verification code, session link, and clipboard/share buttons in-session sheet.

**Progress – 2025-11-04**
- ✅ Desktop app now hosts a “session actions” sheet wired to the native picker selection. Auth controls trigger the Beach Auth device flow (`AuthStatus::Pending`) and persist tokens via the shared credential store/keychain. Successful logins refresh access tokens automatically using `auth::maybe_access_token`.
- ✅ After authentication the app fetches the caller’s private beach inventory via `GET /private-beaches`, surfaces a combo box to pick a target, and remembers the last selection. The control disables gracefully when tokens expire.
- ✅ Clicking “Create public session” invokes `SessionManager::host()` against the configured Beach Road base, storing the new session id/join code, copying them into the UI, and offering one-click Surfer launch + clipboard copy.
- ✅ “Attach session” issues `POST /private-beaches/:id/sessions/attach-by-code` with an embedded `capture_descriptor` payload (target id + base64 ScreenCaptureKit filter/config). It then patches `/sessions/:id` metadata to include `cabana.descriptor`, picker metadata, and the optional nickname.
- ✅ Session log + telemetry entries document login progress, session creation, and private beach attachments for QA.
- 🔬 Manual smoke: exercised mock picker selection → Beach Auth login → Beach Road session creation → private beach attach (mock manager) using the desktop UI; verified descriptors persist in manager metadata.

**Dependencies**
- Works closely with Workstream B (descriptor content for host launch) and Workstream D (viewer contract).

### Workstream D – Beach Surfer Viewer (**In progress**)

**Goals**
1. Provide reusable React components that can render Cabana sessions (public + private) with ergonomic props.
2. Update Beach Surfer routes to use the new components and display real-time streaming/metadata.

**Tasks**
- [x] Design component API (inputs: session id/passcode or private beach id; outputs: playback state, error handling).
- [x] Implement viewer components and wire to existing Noise/WebRTC transport.
- [x] Update public session entry page + private beach page to consume new components.
- [x] Add telemetry hooks + UX polish (loading, secure badges, latency metrics).
- [x] Document usage for other teams.

**Exit criteria**
- Surfer renders Cabana streams in both public and private flows using the new components.
- Components are reusable (documented props, storybook/README optional).
- Telemetry + manual QA checklist complete.

#### 2025-10-21 Progress (Codex)

- Implemented `CabanaSessionPlayer` (`apps/beach-surfer/src/components/cabana/CabanaSessionPlayer.tsx`) to encapsulate WebRTC connection + playback UX. Props now accept `sessionId`, `baseUrl`, optional `passcode`/`viewerToken`, `autoConnect`, `clientLabel`, and telemetry callbacks. The component layers idle/connecting/error overlays, a verification badge, and emits DOM telemetry events (`cabana-viewer:state`, `cabana-viewer:first-frame`, `cabana-viewer:error`, `cabana-viewer:secure`).
- Added `CabanaTelemetryHandlers` so hosts can observe state/first-frame/error/secure updates without reimplementing wiring. Default handlers forward to the DOM events above.
- Created `CabanaPrivateBeachPlayer` (`apps/beach-surfer/src/components/cabana/CabanaPrivateBeachPlayer.tsx`) to request viewer credentials from Beach Manager, normalise passcode vs viewer-token flows, and reuse the public player with pluggable signed-out/loading/error UI.
- Surfaced the new player inside Beach Surfer (`apps/beach-surfer/src/App.tsx`) with memoised telemetry logging, replacing the old `BeachViewer` usage while keeping the terminal fallback available for other surfaces.
- Updated Private Beach tiles (`apps/private-beach/src/components/SessionTerminalPreviewClient.tsx`) to detect Cabana harnesses and mount `CabanaPrivateBeachPlayer`, keeping the terminal preview path as a fallback. Dashboard tiles (`apps/private-beach/src/components/TileCanvas.tsx`) pass the harness metadata through expanded + grid layouts.
- Beach session bridge (`apps/beach-surfer/src/components/BeachSessionView.tsx`) now understands viewer tokens, custom client labels, stream-kind callbacks, and secure summary notifications, enabling the higher-level components.

**Verification (2025-10-21)**
- ✅ Manual TypeScript checks satisfied via editor tooling.
- ⚠️ `pnpm -C apps/beach-surfer test` fails on existing Argon2 WASM path resolution (`/wasm/argon2.wasm`) during vitest; Cabana viewer logic loads without additional regressions.
- Suggested manual QA when the pipeline is live:
  1. Public flow — join a Cabana session via Surfer, confirm overlay progression, secure badge, and `window` telemetry events.
  2. Private beach tile — attach a Cabana session, observe credential fetch spinner, stream start, and first-frame timing.
  3. Error handling — simulate invalid passcode or detached session to validate the new error overlays.


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

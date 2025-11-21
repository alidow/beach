# Unified Transport Refactor Plan

_Last updated: 2025‑11‑20_

## Why this exists

Fast-path currently relies on a separate WebRTC session that the harness spins up
on its own (with `mgr-actions`, `mgr-acks`, and `mgr-state` SCTP channels). That
architecture was expedient but brittle:

- It duplicates signaling (Beach Road `/fastpath/...` endpoints) and forces the
  host CLI to manage extra “peers” that the manager never actually advertises.
- Ack/state channels frequently time out because only the controller handshake
  is truly wired through the server.
- Harness traffic bypasses the core transport pipeline, so observability,
  authorization, and backpressure control are inconsistent.

The long-term goal is a single physical transport between host ↔ manager
↔ browser where all auxiliary streams (actions, acks, state, etc.) are expressed
as typed messages layered on top of the existing wire protocol. This document
breaks that goal into milestones, defines deliverables, and outlines how to test
each step so another agent can implement the plan end-to-end.

## High-level goals

1. **Unify transport**: One negotiated transport (WebRTC or HTTP fallback) per
   host session, shared by browsers and harnesses.
2. **Typed extension frames**: Extend the host wire protocol with a generic
   “extension” envelope so subsystems can send/receive structured payloads
   without inventing new SCTP channels.
3. **Harness integration**: Beach Buggy and manager components tap into the
   transport hooks instead of maintaining independent WebRTC sessions.
4. **Observability & security**: Extension traffic inherits the same telemetry
   and auth as the core transport; we can enforce per-message ACLs at the
   manager layer.

## Non-goals

- Replacing WebRTC altogether. We’re only restructuring how we use it.
- Rewriting the fast-path payload schema (chunking, etc.). We’ll reuse existing
  serialization inside the new extension envelope.
- Redesigning Beach Road’s authorization model beyond what’s required to route
  extension messages.

## Current architecture (summary)

- Host CLI negotiates a primary transport via `SessionManager::host()`; browsers
  attach via `viewer_worker`.
- Harnesses (CLI controller/agent) invoke `FastPathClient`, which opens a second
  RTCPeerConnection to `/fastpath/...` endpoints and creates three SCTP data
  channels for actions/acks/state.
- Manager runs two different workers: `controller_forwarder` (label
  `mgr-actions`) and `viewer_worker` (label `beach-manager`). Only the controller
  label is ever advertised over fast-path signaling; aux peers are not.

Result: we waste bandwidth on duplicate handshakes, ack/state readiness never
completes, and the smoke harness times out.

## Proposed architecture (overview)

1. **Protocol envelope**: Extend the core host protocol
   (`apps/beach/src/protocol/mod.rs` and `apps/beach/src/protocol/wire.rs`) with
   `HostFrame::Extension` / `ClientFrame::Extension` carrying
   `{ namespace: String, kind: String, payload: bytes/json }`.
2. **Transport hooks**: Extend `Transport` (`WebRtcTransport`, `HttpTransport`)
   with `send_extension()` and namespace-aware subscriptions so extension
   traffic rides the single negotiated transport. The transport may still use
   multiple SCTP channels internally for QoS, but callers never spin up extra
   peer connections.
3. **Harness refactor**: Update Beach Buggy to register as an extension handler
   and send fast-path payloads via those hooks.
4. **Manager integration**: Update `controller_forwarder`/`viewer_worker` to use
   the extension APIs, eliminating second-class signaling.
5. **Legacy removal**: Delete `/fastpath/...` endpoints, fake `mgr-acks`
   peers, and any per-channel plumbing once extension traffic is stable.

## Milestones & testing strategy

### Milestone 0 – Baseline instrumentation

**Deliverables**
- Document current fast-path flows (already captured in
  `docs/private-beach/pong-fastpath-ack-state-investigation.md` and the logs
  referenced there).

**Testing**
- No automated testing for this milestone; the goal is documentation and
  instrumentation notes only. Capture references to existing logs in this doc
  for future comparison.

### Milestone 1 – Extension envelope definition

**Implementation steps**
1. Update `beach_client_core::protocol::{HostFrame, ClientFrame}` plus the
   binary codec in `apps/beach/src/protocol/wire.rs` to add typed extension
   variants. This is the canonical schema for all consumers:

   ```rust
   pub struct ExtensionFrame {
       pub namespace: String, // e.g. "fastpath"
       pub kind: String,      // e.g. "action", "ack", "state"
       pub payload: Bytes,    // raw bytes or serialized JSON
   }
   ```

2. Update `HOST_KIND_*` / `CLIENT_KIND_*` discriminants in
   `apps/beach/src/protocol/wire.rs` so the new frame is encoded/decoded
   explicitly.
3. Update protocol docs (`docs/beach-client-spec.md`) to describe the envelope.

**Testing**
- Add unit tests for serde round-trips.
- Manual verification: run `cargo test -p beach-buggy fast_path::tests::parses_*`
  to ensure no regressions.

### Milestone 2 – Transport-layer hooks

**Implementation steps**
1. Extend `beach_client_core::transport::Transport` (and its concrete
   implementations in `apps/beach/src/transport/webrtc/mod.rs` and
   `apps/beach/src/transport/websocket.rs`) with:
   - `async fn send_extension(&self, frame: ExtensionFrame, lane: ExtensionLane) -> Result<()>`
   - `fn subscribe_extensions(&self, namespace: &str) -> broadcast::Receiver<ExtensionFrame>`
2. Wire the new APIs into the Rust transports. Expose explicit
   reliability/priority lanes so controllers cannot be starved by bulk terminal
   frames:
   - `ExtensionLane::ControlOrdered` (reliable/ordered) for actions/acks.
   - `ExtensionLane::StateUnordered` (best-effort) for state diffs.
   The transport decides whether to back lanes with dedicated SCTP channels or
   prioritized queues; callers simply select the lane when sending.
3. Add tracing counters for sent/received extension frames
   (`metrics::TRANSPORT_EXTENSION_FRAMES`).

**Testing**
- Unit tests: use a dummy transport to ensure subscribers see extension frames.
- Integration: run `cargo watch -x 'test transport::webrtc'`.

### Namespace scoping & ACLs

Extension frames must be scoped so viewers never receive controller payloads.
Implement an `ExtensionRegistry` inside the manager core
(`apps/beach-manager/src/state.rs`):

- Each namespace declares which roles (host, controller, viewer, agent) may
  publish or subscribe. `fastpath` is host↔manager only.
- When a controller transport is negotiated, register its permissions with the
  registry; `transport.subscribe_extensions(namespace)` consults the registry
  before enqueuing frames.
- Unauthorized publishes are rejected server-side and logged (`metrics::
  TRANSPORT_EXTENSION_DENIED`), preserving the current security posture.
- Document namespace policies so downstream services know which hooks are
  available.

### Milestone 3 – Harness migration

**Implementation steps**
1. Provide Beach Buggy with a bridge to `beach_client_core::transport::Transport`.
   Two viable paths:
   - Introduce a lightweight crate (e.g. `beach-transport-bridge`) that wraps an
     `Arc<dyn Transport>` and implements Buggy’s `ManagerTransport` trait on top
     of it.
   - Or make Beach Buggy depend on `beach_client_core` directly and reuse the
     transport trait from there.
   Pick one approach and document it here. Either way, we need to expose the
   host’s negotiated transport handle (inside `apps/beach/src/server/terminal`
   when `OffererAcceptedTransport` is yielded) so the harness can call
   `FastPathClient::attach(transport.clone())`.
2. Map existing fast-path payloads to extension frames:
   - `namespace = "fastpath"`, `kind = "action"` when sending commands.
   - `kind = "ack"` / `kind = "state"` for responses.
3. Adjust Beach Buggy consumers (`ManagerTransport::send_actions`,
   `ManagerTransport::send_state`, etc.) to use the new bridge rather than the
   bespoke `HttpTransport`/WebRTC code paths. HTTP fallback remains the
   manager’s responsibility; extension frames ride on whatever transport the
   host already negotiated with the manager.

**Testing**
- Unit: mock transport to ensure actions/acks route through the hooks.
- Regression: re-run harness tests (`cargo test -p beach-buggy fast_path`).
- Defer end-to-end smoke testing until the manager wires the extension hooks in
  Milestone 4.

### Milestone 3 – Status

- **Code**: Added a unified harness bridge that adapts the negotiated transport
  to Beach Buggy’s `ManagerTransport`/`ControllerTransport`, mapping fast-path
  actions/acks/state/health to extension frames (`apps/beach/src/transport/unified_bridge.rs`).
  Exposed fast-path action decoding for reuse in the bridge (`crates/beach-buggy/src/fast_path.rs`).
- **Tests**: `cargo test -p beach transport::unified_bridge` ✔️ (actions + ack/state
  extension flow); `cargo test -p beach-buggy fast_path` ✔️.
- **Open TODOs**: Wire the bridge into host/harness orchestration (surface the
  negotiated transport handle when the controller connects) and add lane/QoS
  handling once manager-side extension routing is implemented.

### Milestone 4 – Manager/controller integration

**Implementation steps**
1. Update `controller_forwarder_once_with_label` and `viewer_worker` to call
   `transport.subscribe_extensions("fastpath")` (controllers only) and handle
   extension frames. Viewers continue to subscribe only to whitelisted
   namespaces.
2. Implement routing inside `apps/beach-manager/src/state.rs`:
   - `fastpath.actions` → `ack_actions` queue.
   - `fastpath.state` → `record_state`.
3. Expose a public API (REST or GraphQL) for other services (e.g. agent tiles)
   to publish extension frames if needed.
4. Remove the fake `mgr-acks`/`mgr-state` peer logic from the host once the
   manager handles extension frames.

**Testing**
- Unit: simulate extension frames in state.rs tests.
- Integration: once both CLI and manager milestones are complete, run
  `scripts/pong-fastpath-smoke.sh --skip-stack` and expect the test to pass
  (fast-path ready for lhs/rhs). Capture logs/artifacts.
- Regression: run `scripts/fastpath-smoke.sh` to cover any remaining legacy
  flows that still expect the old endpoints until Milestone 5 removes them.

### Milestone 4 – Status

- **Code**: Manager now both parses and emits fastpath traffic via extension frames:
  controller actions are sent over `ExtensionFrame` with ordered lanes, with automatic
  fallback to legacy frames if the channel drops; ack/state/health extensions are
  ingested and recorded (`apps/beach-manager/src/state.rs`). Viewer/forwarder
  host-frame handling tolerates extension frames.
- **Tests**: `cargo check -p beach-manager` ✅ (compiles; no targeted unit tests run in this pass).
- **Open TODOs**: Finish the unified pipeline by cleaning up legacy fast-path
  plumbing (Milestone 5), add focused unit/integ tests, and run the deferred smokes.

### Milestone 5 – Decommission legacy fast-path session

**Implementation steps**
1. Retire `FastPathSession` in `apps/beach-manager/src/fastpath.rs` by
   reimplementing its logic as handlers for `ExtensionFrame`s (actions/acks/
   state). Once all lanes are unified, delete the fast-path-specific RTCPeer
   connection, ICE plumbing, and `/fastpath/...` REST endpoints.
2. Drop the CLI code that manufactures extra peers (`mgr-acks`/`mgr-state`).
   Remove `controller_fast_path_enabled`/`CONTROLLER_FAST_PATH_*` toggles once
   the extension pipeline is the default.
3. Clean up metrics and docs referencing the old architecture.

**Testing**
- Run both smoke tests once more to confirm the legacy `/fastpath` path is gone
  and unified transport continues to pass.
- Perform load/regression testing on staging: open multiple sessions, confirm
  controllers and viewers stay synchronized.

### Milestone 5 – Status

- **Code**: Host/controller attach now prefers unified transport when the manager
  advertises extensions: negotiated single transports instantiate
  `UnifiedBuggyTransport` with a fastpath extension subscription, state/health/idle
  publishers attempt extension sends first with HTTP/legacy fast-path fallback, and
  incoming fastpath extensions are ingested for action routing/metrics. Manager transport
  hints now advertise `extensions.namespaces=["fastpath"]` to gate the unified path while
  legacy peers/endpoints remain as fallback. Host-side viewers explicitly ignore fastpath
  extensions; counters were added for extension send/receive/fallback.
- **Tests**: `cargo check -p beach`, `cargo check -p beach-manager`, `cargo test -p beach transport::unified_bridge`.
- **Open TODOs**: Run the fastpath/pong smokes (unified on/off), then delete mgr-acks/mgr-state
  peer creation and remaining fast-path metrics/toggles/docs once confidence is established.

## Additional notes & open questions

- **Security**: the `ExtensionRegistry` described above enforces namespace ACLs.
  Consider tagging frames with the controller token/lease ID for auditing.
- **Observability**: add log lines and metrics for dropped/invalid extension
  frames. We should be able to answer “how many fast-path actions arrived via
  unified transport vs HTTP?”.
- **Backpressure**: the `ExtensionLane` QoS lanes ensure controller traffic
  stays responsive. Add Prometheus gauges for lane depth to detect starvation.
- **Protocol shape**: we chose a flexible `{namespace, kind, payload}` envelope
  to accommodate future subsystems. If implementation turns out to be simpler
  with dedicated `HostFrame::FastPath{...}` variants, call that out in the
  Milestone 1 PR and adjust accordingly.
- **Scope clarity**: “single transport” means “each participant reuses its
  existing negotiated transport (WebRTC/WebSocket/HTTP fallback) for controller
  traffic,” not that every participant shares the same physical channel. Hosts,
  browsers, and agents still negotiate their own transports; they simply stop
  opening extra `/fastpath` sessions.

## Handoff checklist

When picking up this project:

1. Start at Milestone 1 and ensure PRs include unit tests + smoke test notes.
2. Update this document after each milestone with links to commits and test
   artifacts so future agents know the current state.
3. Coordinate with infra to retire `/fastpath/...` load balancer rules once the
   final milestone is complete.

With this plan, another Codex instance (or human teammate) should have enough
context to implement the unified transport architecture from start to finish.***

### Milestone 1 – Status

- **Code**: Added the shared `ExtensionFrame` schema plus host/client variants
  and binary codec support (`apps/beach/src/protocol/{mod,wire}.rs`,
  `apps/beach/src/lib.rs`, `apps/beach/Cargo.toml`). Updated consumers to treat
  extension traffic as ignorable for now (client/server/transport tests) and
  documented the envelope in `docs/beach-client-spec.md`.
- **Tests**: `cargo test -p beach encode_decode_extension_frames` ✔️;
  `cargo test -p beach-buggy parses_` ✔️.
- **Open TODOs**: Milestone 2 needs to surface real transport hooks (send /
  subscribe lanes) so extension frames can be emitted and observed rather than
  ignored.

### Milestone 2 – Status

- **Code**: Added transport-layer extension APIs and namespace routing
  (`apps/beach/src/transport/{mod,extensions}.rs`) plus publication hooks where
  extension frames are decoded (client/server handlers). The registry now
  delivers frames per transport id + namespace; lanes are plumbed via
  `ExtensionLane`, ready for prioritized channels later.
- **Tests**: `cargo test -p beach transport::extensions::tests` ✔️.
- **Open TODOs**: Wire real lane QoS/metrics inside WebRTC/WebSocket transports
  and start emitting extension frames from higher-level fast-path flows (Milestone 3).

### Milestone 2 – Status

- **Code**: Added transport-level extension hooks and plumbing: new send/subscribe
  APIs with lane enums (`apps/beach/src/transport/mod.rs`), an in-process
  extension bus (`apps/beach/src/transport/extensions.rs`), and publication
  points in the client/server handlers so callers can subscribe by namespace.
  Extension frames now flow through the binary codec for both directions.
- **Tests**: `cargo check -p beach` ✔️; codec round-trips covered by
  `cargo test -p beach encode_decode_extension_frames`.
- **Open TODOs**: Wire transports to actual lanes/QoS, and migrate Beach Buggy +
  manager to consume the subscription API (Milestones 3–4). No end-to-end smoke
  runs yet (deferred per updated plan).

### Milestone 3 – Status

- **Code**: Implemented the Beach Buggy bridge atop unified transport:
  `UnifiedBuggyTransport` maps manager actions/acks/state/health onto extension
  frames (`apps/beach/src/transport/unified_bridge.rs`), uses the common codec,
  and publishes non-fastpath extensions to the namespace bus for other
  subscribers. This adapts any negotiated `Arc<dyn Transport>` without spinning
  additional RTC peers.
- **Tests**: `cargo test -p beach transport::unified_bridge` (bridge unit tests)
  ✔️; fast-path parser coverage via `cargo test -p beach-buggy parses_` ✔️.
- **Open TODOs**: Swap Beach Buggy consumers over to the bridge in the host
  lifecycle and keep HTTP/WebRTC fast-path as fallback until manager integration
  (Milestone 4). End-to-end smoke remains deferred.

### Milestone 5 – Notes (prep)
- Extension handling observability: `UnifiedBuggyTransport` now traces received
  extension frames (namespace/kind/len) and ignores unknown namespaces; added
  `ignores_unknown_extension_namespace` test in
  `apps/beach/src/transport/unified_bridge.rs`.
- Tests run: `cargo test -p beach transport::unified_bridge` (covers the new
  ignore-unhandled test).

### Milestone 5 – Status

- **Code**: Host now prefers unified transport for controller traffic: negotiated
  transports are wrapped in `UnifiedBuggyTransport` (with subscription mode),
  controllers subscribe to `fastpath` extensions, and unified action/state/health
  consumers run alongside legacy fallback. State/health/idle publishers attempt
  unified extension sends first, then legacy fast-path channel, then HTTP. Host
  metrics added (`transport_extension_{sent,received,fallback}_total`) and viewer
  paths drop `fastpath` extensions. Manager hints advertise
  `extensions.namespaces=["fastpath"]` to gate unified usage.
- **Tests**: `cargo check -p beach` ✔️; `cargo check -p beach-manager` ✔️;
  `cargo test -p beach transport::unified_bridge` ✔️.
- **Open TODOs**: Run deferred smoke tests (unified on/off) and, if stable,
  delete residual legacy fast-path plumbing (stub module remains). Update docs
  with smoke results when available.

### Milestone 4 – Status

- **Code**: Manager controller forwarder now publishes incoming fast-path
  extensions to the namespace bus and subscribes to `fastpath` extensions via
  `transport.subscribe_extensions`, routing ack/state/health into the existing
  event loop (`apps/beach-manager/src/state.rs`). This makes controller traffic
  consumable over the unified transport without opening extra peers.
- **Tests**: `cargo check -p beach-manager` ✔️.
- **Open TODOs**: Keep legacy fast-path/HTTP fallback active while we migrate
  hosts; add metrics/tracing for extension routing and wire viewers only to
  whitelisted namespaces. Smoke tests still deferred until manager + host are
  both unified.

### Milestone 5 – Host/harness migration & legacy removal (plan)

A detailed implementation plan lives in
`docs/unified-transport/host-unified-migration.md`. Summary of next actions:
- Wire `UnifiedBuggyTransport` into the host negotiation flow; prefer unified
  transport, fallback to HTTP/legacy fast-path if extensions unsupported.
- Scope extension subscriptions/metrics; viewers should not consume fastpath.
- Once unified passes smoke tests in both modes, remove legacy fast-path peers
  and `/fastpath/...` endpoints, and clean up related metrics/docs.

Legacy removal checklist: see `docs/unified-transport/legacy-removal-plan.md`
for a step-by-step breakdown of host/manager changes, observability, and test
matrix before deleting the fast-path stack.

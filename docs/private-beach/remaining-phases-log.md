# Remaining Phases Execution Log

> Running notes while delivering milestones defined in `docs/private-beach/remaining-phases-plan.md`.  
> Keep entries chronological with date + author; capture decisions, verification steps, and outstanding risks.

## 2025-06-19 — Kickoff (Codex)

### Milestone M1 — Surfer UX Foundations
- [x] Draft UX kickoff brief:
  - Outline IA/navigation assumptions, component system targets, accessibility/performance acceptance bars.
  - Identify existing Surfer components that become shared primitives.
- [x] Create tracking issue list for design system + auth migration work (`docs/private-beach/ux-foundation-issues.md`).
- [ ] Document verification plan (axe-core/lighthouse runs) once implementation begins.

### Milestone M2 — Orchestration Mechanics (Fast-Path Prototype)
- [x] Survey current `beach-buggy` capabilities and identify integration points for fast-path data channels.
- [x] Add harness-side fast-path client scaffold (WebRTC negotiation + channel handlers).
- [x] Implement initial handshake: harness fast-path client now negotiates SDP/ICE and exposes action broadcast + ack/state send helpers.
- [x] Thread `FastPathConnection` into harness runtime with HTTP fallback for actions/acks/state (`cargo test -p beach-buggy`).
- [x] Manager receive loops ingest fast-path acks/state and emit Prometheus counters (`cargo test -p beach-manager`).
- [ ] Capture validation approach:
  - Unit tests with mocked data channel.
  - Manual recipe aligning with STATUS “Manual Fast-Path Test”.

### Cross-Cutting
- [x] Update `docs/private-beach/STATUS.md` once initial scaffolding lands.
- [x] Capture fast-path validation plan + TURN/STUN knobs (`docs/private-beach/fast-path-validation.md`).

## 2025-06-24 — Controller Pairing MVP (Codex)

### Milestone M2 — Orchestration Mechanics
- [x] Added `controller_pairing` persistence (enum-backed cadence, RLS policies, Prometheus gauge/counter) with in-memory fallback mirroring Postgres semantics.
- [x] Landed REST + MCP surfaces for pairing CRUD + SSE stream (`/sessions/:controller_id/controllers[/stream]`), enforcing active controller lease + beach membership.
- [x] Extended `AppState` broadcast plumbing so pairing events ride the existing session channels and surface in metrics/logs.

### Harness
- [x] `beach-buggy` now ships a controller runtime: auto-renews leases, consumes pairing SSE (with HTTP fallback), and logs prompt/cadence + transport status per child session.
- [x] Added `ControllerTransport` abstraction, HTTP/in-memory adapters, and covered the runtime with async tests (`cargo test -p beach-buggy`).

### Docs & Validation
- [x] Refreshed `fast-path-validation.md` with pairing smoke steps (REST/SSE checks, harness log expectations, Prometheus gauges).
- [x] Logged REST regression coverage for pairing CRUD (`cargo test -p beach-manager`).

## 2025-06-26 — Controller Pairing UI Polish (Codex)

### Private Beach UI
- [x] Swapped the temporary pairing stubs for the live Manager REST endpoints and subscribed to `/sessions/:controller_id/controllers/stream` so tiles, badges, and the modal refresh on SSE payloads.
- [x] Added drag/drop overlays (“Drop controller here” → “Release to pair”) plus kept the Pair button as the accessible fallback; modal now stays in sync when pairings are edited or removed elsewhere.
- [x] Patched the tile canvas to capture drops over the terminal preview (overlay intercepts iframe drops) and highlighted transport status changes instantly.

### Tests & Tooling
- [x] Introduced `useControllerPairingStreams` hook with Vitest coverage that mocks `EventSource` end-to-end.
- [x] Added a lightweight Playwright spec exercising the stream reducer (`applyControllerPairingEvent`) so CI can validate SSE merges in both runners.

### Docs
- [x] Updated `STATUS.md` and `fast-path-validation.md` with the new streaming UX details and validation steps.

## 2025-07-02 — Controller Pairing Transport Telemetry (Codex)

### Backend
- [x] Added typed `PairingTransportStatus` to pairing payloads/SSE so fast-path vs HTTP fallback (latency + last error) is visible end-to-end.
- [x] Updated `queue_actions`/`ack_actions` to record transport transitions, publish `"updated"` pairing events, and keep the Prometheus gauges/counters in sync.

### Harness
- [x] Mirrored the transport status shape in `beach-buggy`, enriched controller pairing logs with transport/latency/error fields, and kept the SSE→poll fallback behaviour intact.
- [x] Added unit tests covering pairing snapshot/upsert handling so the controller runtime preserves status metadata between events.

### Docs & Validation
- [x] Expanded `fast-path-validation.md` with transport-focused checks (SSE expectations, harness log cues, `controller_pairings_events_total{action="updated"}`).

## 2025-07-04 — Agent/Application UX Blueprint (Codex)

### Product & UX
- [x] Drafted `docs/private-beach/controller-agent-ux-plan.md` capturing the new agent/application terminology, tile redesign, explorer-based assignment workflow, and mobile considerations.
- [x] Replaced modal-first editing with a right-hand assignment pane in the plan, ensuring future engineers have a concrete roadmap.
- [x] Logged accessibility, data-model, and responsive design requirements so follow-on implementation can proceed without additional discovery.

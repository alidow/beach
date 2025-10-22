# Private Beach WebRTC Refactor — Detailed Handoff Plan

## Context & Current State (June 2025)
- The Private Beach stack previously mirrored terminal state to Beach Manager over an **HTTP harness** (Beach Buggy) that registered the host, pushed diffs via REST/SSE, and kept the dashboard alive even when no browsers were connected. That path has now been retired; Manager consumes diffs via its WebRTC viewer and persists them directly.
- This shortcut unblocks demos, but it breaks the Beach philosophy:
  - Adds latency (HTTP/SSE buffering) compared with direct WebRTC viewers.
  - Leaks private-beach concepts into every public host (bridge tokens, auto-registration).
  - Scales poorly—every byte funnels through Manager even when millions of agents/viewers should talk peer-to-peer.
- The dashboard tiles also read from Manager’s SSE streams, so they no longer reuse the proven Beach Surfer components.
- We have now aligned on a **WebRTC-first architecture** where:
  1. **Manager joins sessions exactly like any other Beach viewer**, using the same negotiation code as the CLI.
  2. Manager persists an audit cache from that viewer feed so it can serve automation, recordings, and compliance—even when no browser is attached.
  3. Beach Surfer (React tiles) once again talks directly to hosts (WebRTC/TURN), with Manager only brokering credentials.
  4. **Beach Buggy becomes optional**, providing derived/semantic transforms only when explicitly requested. It never owns the primary diff stream.

> **Non‑Goals:** Do not add new SSE endpoints, HTTP diff pumps, or manager-in-the-middle features. Avoid introducing new secrets/flows that teach public hosts about private beaches beyond passing viewer credentials.

## Guiding Principles
- **WebRTC is the golden path.** TURN/WSS are paid fallbacks gated by Beach Gate entitlements; if a user lacks entitlement we fail fast with a helpful error. No automatic downgrade to HTTP.
- **Reuse existing client code.** The `apps/beach` CLI already handles negotiation, diffs, contention, TURN, and encryption. We expose this as a library instead of re-implementing a second protocol inside Manager.
- **Manager remains the audit source of truth.** It must capture every diff and action for compliance even if no browser is open. That capture now happens through Manager’s own viewer instance, not via injected harness logic.
- **Harness is opt-in enrichment.** It listens to the host’s stream and emits semantic overlays (OCR, motion vectors, summaries) when a client opts in. It never reroutes a request or arbitrates controller contention.
- **Documentation-first.** Each legacy doc mentioning HTTP/SSE/bridge harness flows must flag them as deprecated so future work does not regress.

## Current Progress & Handoff Summary (June 2025)
- **Shared client crate** — `apps/beach` exports the `beach_client_core` library while the package/bin remain `beach`. Negotiation helpers, terminal cache, and protocol types are reusable, and all existing unit tests import from `beach_client_core::…`.
- **TURN entitlement check** — WebRTC negotiation fails fast if the caller lacks `pb:transport.turn`; we no longer hit HTTP/SSE fallbacks silently. STUN-only paths still operate for non-entitled users.
- **Manager viewer worker (authoritative)** — `AppState::spawn_viewer_worker` now negotiates WebRTC, decodes frames, and persists them to both Redis and `session_runtime` while emitting `StreamEvent::State`. Metrics (`manager_viewer_connected`, `manager_viewer_latency_ms`, `manager_viewer_reconnects_total`, `manager_viewer_keepalive_sent_total`, `manager_viewer_keepalive_failures_total`, `manager_viewer_idle_warnings_total`, `manager_viewer_idle_recoveries_total`) track health, the worker auto-reconnects until shut down, and a periodic `__keepalive__` ping avoids silent ICE idling while logging when hosts stop sending frames.
- **Credential plumbing & API** — `RegisterSessionRequest` still persists an optional `viewer_passcode` for handshake compatibility, but `GET /private-beaches/:bid/sessions/:sid/viewer-credential` now always returns a short-lived Gate-signed viewer token. Browsers consume the token directly, and Beach Road verifies it before completing the join (no more passcode fallback).
- **Legacy harness removed** — Manager no longer exposes the HTTP pump (`handle_manager_hints`). The viewer worker now publishes directly to Redis and `session_runtime`, enabling HTTP bridge code to be deleted from the host.
- **CLI host cleanup** — `apps/beach` no longer listens for manager bridge hints or pushes HTTP diffs; the WebRTC path is the only authority. Bridge-token mint/nudge endpoints were deleted from Beach Road and Manager.
- **Dashboard viewer parity** — Private Beach tiles render via the shared Beach Surfer `BeachTerminal` with security/latency badges and reconnect messaging surfaced inline. Cabana sessions continue to flow through `CabanaPrivateBeachPlayer`, drawers pull from `GET /sessions/:id/controller-events`, and the legacy `/sessions/:id/events/stream` SSE endpoint has been removed.
- **Docs plan status** — Phase 0 tasks are complete (crate extraction ✅, entitlement audit ✅, credential design ✅). Earlier sections note status for quick scan.
- **Open risks / follow-ups**
  1. Observer diff pipeline — ✅ style updates now survive the viewer pipeline (new integration test). We can still add a multi-session Redis smoke test if we want additional coverage.
  2. TURN-only validation — Manual testing documented (`BEACH_WEBRTC_DISABLE_STUN=1`); schedule a sustained TURN soak test to baseline quota impact before GA.
  3. Harness transforms — After transport parity, re-scope Beach Buggy to opt-in transforms with dedicated data channels; HTTP endpoints remain removed.
- **Quick verification** — `cargo check -p beach-manager` still fails because of the pre-existing lifeguard fallback token drift (unchanged). `npx tsc --noEmit` in `apps/private-beach` now succeeds after raising the TS target to ES2020 and adding module stubs. Running `npm run lint` currently prompts for the Next.js ESLint scaffold; set `CI=1` if we decide to wire their preset in CI.
- **Incoming engineer gameplan**
  - Layer a Redis-backed viewer worker smoke test if we decide the current unit coverage is insufficient.
  - Run the TURN-only soak outlined above and feed results into transport budgeting.
  - Keep the harness transforms item alive once transport polish winds down.

## Architecture Overview After Refactor
```
┌───────────┐        WebRTC (Surfer credentials)        ┌──────────────┐
│ Browser   │ ───────────────────────────────────────▶ │ Host (CLI)   │
│  tiles    │                                          │ apps/beach   │
└───────────┘                                          └─────┬────────┘
        ▲                                                       │
        │ WebRTC (Manager viewer token)                         │
┌───────┴────────┐   Audit cache, automation, agents            │
│ Beach Manager  │ ◀────────────────────────────────────────────┘
│ (Rust)         │
└───────┬────────┘
        │ optional transform channels
┌───────┴────────┐
│ Beach Buggy    │ (semantic streams only, opt-in)
└────────────────┘
```

## Migration Phases & Deliverables

### Phase 0 – Preparation (Now)
1. **Crate extraction**
   - Status: ✅ crate exports shared negotiation/viewer APIs via the `beach_client_core` library while retaining the `beach` package for CLI compatibility.
   - Add `apps/beach/src/lib.rs` that exposes:
     - Session negotiation (`negotiate_transport`, `SignalingClient`, TURN helpers).
     - Terminal diff reader (`TerminalGrid`, `terminal::viewer`).
     - Transport interfaces used by Beach Surfer.
   - Keep `main.rs` as the CLI entry point; binary links the shared lib.
2. **Credential story**
   - Status: ✅ Manager mints Gate-signed viewer tokens and the Private Beach UI consumes them directly; passcode fallback to browsers was removed (passcodes are only stored server-side for handshake/backfill).
   - API summary: `GET /private-beaches/:id/sessions/:sid/viewer-credential` issues a short-lived viewer token; Beach Road verifies it before completing the join.
3. **Entitlement audit**
   - Status: ✅ TURN fallback now errors when `pb:transport.turn` missing and only STUN fallback continues.
   - Ensure TURN/WSS fallback path checks entitlements. If a user lacks `pb:transport.turn`, we reject rather than silently downgrade.
   - Remove or feature-flag any HTTP fallbacks in the host.

### Phase 1 – Manager as WebRTC Client
- Status: ✅ manager viewer worker is authoritative (records diffs/metrics, emits `StreamEvent::State`); legacy HTTP harness removed.
Deliverables:
- `manager-client` module consuming the shared `beach_client_core` library.
- Manager service spawns a lightweight “viewer worker” per attached session:
  - Joins via Beach Road signaling using stored credential.
  - Streams diffs into the existing cache (`session_runtime` row + Redis).
  - Exposes health/status so the dashboard knows Manager is ingesting data even if no browsers are connected.
- Remove calls to the HTTP frame pump in Manager (feature flag `LEGACY_HTTP_HARNESS=false`).
- Provide metrics: `manager_viewer_connected`, `manager_viewer_latency_ms`, `manager_viewer_reconnects_total`.

Testing:
- End-to-end attach flow with a real public session: Manager should join automatically, log diffs, and persist them.
- Kill the manager viewer process—ensure it auto-reconnects without nudging the host.
- Verify no HTTP `/sessions/:id/state` POST calls originate from hosts.

### Phase 2 – Dashboard Parity
Deliverables:
- Refactor tiles to use the real Beach Surfer viewer component:
- Manager endpoint `GET /private-beaches/:id/sessions/:sid/viewer-credential` returns the credential (passcode today, token later).
- Frontend spins up the shared terminal viewer with WebRTC, identical to Beach Surfer.
- Replace SSE bridge code (`ManagerTerminalFeed`, HTTP diff patches) with WebRTC previews/drawer views.
- Update layout/UX docs to reflect pure WebRTC streaming.
- Keep Manager’s cached state for offline queries (e.g., command history) but do not rely on it for live rendering.

Testing:
- Attach session, open dashboard tile and standalone Beach Surfer—ensure both use WebRTC and show the same state.
- No SSE requests should appear in the network tab once refactor is complete.
- Dashboard should still render (using cached snapshots) if no browser is connected and we later reconnect.

### Phase 3 – Harness Hardening (Optional Opt-In)
Deliverables:
- Re-scope Beach Buggy per updated spec:
  - Only runs when a consumer requests transforms.
  - Attaches to existing peer connection and publishes new data channels (`mgr-semantic-state`, etc.).
  - No command queue or diff responsibilities.
- Provide API (`POST /sessions/:sid/transforms`) to enable/disable transforms.
- Document capability entitlements (`pb:transform.ocr`, `pb:transform.motion`).

Testing:
- Request a transform; ensure harness spawns and publishes on the new channel.
- Disable transform; harness shuts down gracefully.
- Ensure public sessions without requested transforms never load the harness.

### Phase 4 – Scale, Observability, Rollout
Deliverables:
- Stress test with synthetic load (many hosts, many manager/browsers) to validate WebRTC scaling and TURN budget.
- Instrument:
  - WebRTC join success/failure.
  - TURN minutes consumed, fallback counts.
  - Manager cache lag vs. host diff timestamp.
- Ops notes (documented in STATUS):
  - Force TURN-only verification locally via `BEACH_WEBRTC_DISABLE_STUN=1` on Manager + hosts; confirm the keepalive metrics stay quiet while negotiating over TURN.
  - Monitor the new viewer metrics (`manager_viewer_connected`, `manager_viewer_latency_ms`, `manager_viewer_reconnects_total`, `manager_viewer_keepalive_*`, `manager_viewer_idle_*`) and wire dashboards/alerts accordingly.
- Rollout plan:
  - Feature flag to keep legacy HTTP path for emergency rollback only.
  - Migration script to disable HTTP path across environments once WebRTC viewer stable.

## Immediate To-Do (next sprint)
1. ✅ Add an automated smoke test for `spawn_viewer_worker` (mock session, assert Redis + state stream).
2. ✅ Define and implement the signed viewer credential contract (Gate + Beach Road validation).
3. ✅ Dashboard parity polish landed: tiles show latency/security badges, reconnect messaging, and the session drawer uses the REST feed; `/sessions/:id/events/stream` was removed.
4. ✅ Ops guidance captured (TURN-only test recipe + viewer metrics to monitor).
5. ✅ `npx tsc --noEmit` passes after adding module stubs and bumping the TS target to ES2020.

## Risks & Mitigations
- **Manager load increases** (now running N viewer clients): isolate viewer workers, cap concurrency, and rely on TURN quotas. Mitigate via autoscaling and instrumentation before rollout.
- **Credential exposure**: ensure viewer tokens are scoped and short-lived; never return raw passcodes to browsers unless absolutely needed (prefer viewer JWT).
- **Hosts without refactored binaries**: require an updated CLI that advertises WebRTC viewer support; publish upgrade guidance and verify older harness builds fail fast with a helpful error.
- **Downstream tooling expecting SSE**: audit consumers (CLI tests, scripts) and provide migration. Mark SSE endpoints as deprecated with removal date, now that tiles run entirely on WebRTC.

## How to Onboard the Next Engineer/Instance
1. **Read this plan** plus:
   - `docs/private-beach/vision.md` (updated transport section),
   - `docs/private-beach/beach-buggy-spec.md` (new harness scope),
   - `docs/private-beach/pong-demo.md` (demo expectations with new architecture),
   - `docs/private-beach/STATUS.md` for current implementation checkpoints.
2. Understand that anything mentioning HTTP/SSE in older docs is **legacy/deprecated**. Do not implement new features on those paths.
3. Start with Phase 0 tasks; open tracking issues/PRs for crate extraction and credential design. Keep the feature flag in mind.

## Success Criteria
- Manager can ingest diffs for every attached session purely via WebRTC.
- Browser tiles and Surfer reuse the same viewer component—no HTTP diff bridges.
- Harness only runs when requested and never interferes with base transport.
- Private Beach still maintains a centralized audit cache, even with zero browsers online.
- HTTP/SSE code paths can be removed without regressing demos or automation.

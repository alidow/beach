# Private Beach — Implementation Status (Handoff)

This document captures what’s built, how to run it locally, what’s left, and how to resume implementation quickly.

## TL;DR
- Manager (Rust) persists sessions, leases, and controller events in Postgres; commands/health/state use Redis Streams + TTL caches; the manager viewer worker (WebRTC) is authoritative and `/sessions/:id/events/stream` has been removed (only the state SSE remains as legacy fallback). RLS is enforced via GUC.
- Surfer/Private Beach (Next.js) are Clerk-gated, stream live previews through the shared `BeachTerminal`, and now surface security/latency badges plus reconnect messaging. Cabana sessions still use the media player, drawers poll `controller-events` over REST, and browsers fetch Gate-signed viewer tokens from Manager instead of passcodes.
- Controller assignments now use the agent ↔ application explorer: sessions declare their role (toggleable), applications can be dragged onto agents or assigned via the sidebar, and the right-hand pane replaces the old modal for editing prompts/cadence/transport state.
- Front-end coverage exercises the SSE flow via mocked `EventSource` streams (Vitest + Playwright), ensuring the explorer tree, tile assignment bars, and the detail pane stay in sync with backend events.
- Fast‑path (WebRTC) is scaffolded in the manager with answerer endpoints and routing to send actions over data channels when available. Harness‑side fast‑path client is next.
- WebRTC refactor plan captured in `docs/private-beach/webrtc-refactor/plan.md`; the HTTP frame pump has been retired in favor of Manager joining sessions as a standard Beach client.

## New: Session Onboarding (Attach) — Current Status
- Manager endpoints implemented:
  - `POST /private-beaches/:private_beach_id/sessions/attach-by-code`
  - `POST /private-beaches/:private_beach_id/sessions/attach`
  - `POST /private-beaches/:private_beach_id/harness-bridge-token`
- Beach Road endpoints implemented:
  - `POST /sessions/:origin_session_id/verify-code` (verifies short code)
  - `GET /me/sessions?status=active` (lists caller-owned active sessions)
  - `POST /sessions/:origin_session_id/join-manager` (nudge; no-op for now)
- Surfer UI: “Add Session” modal with tabs (By Code | My Sessions | Launch New). Uses the above Manager/Road routes.
- Defaults: Manager now defaults `BEACH_ROAD_URL` to `https://api.beach.sh` if unset. In docker-compose, it’s explicitly set to the local `beach-road` service.
- Dev bridge tokens: Manager mints an opaque UUID (dev/auth-bypass). Replace with a Beach Gate–minted JWT in prod.
- Tests: Two temp scripts validate attach flows locally: `temp/test-attach-by-code.mjs`, `temp/test-attach-owned.mjs`.

## Runbook

1) Dependencies
- `docker compose up postgres redis`
- Environment:
  - `DATABASE_URL=postgres://postgres:postgres@localhost:5432/beach_manager`
  - `REDIS_URL=redis://localhost:6379`
  - Clerk auth (Manager): `BEACH_GATE_JWKS_URL=<clerk-jwks>`, `BEACH_GATE_ISSUER=<clerk-issuer>`, `BEACH_GATE_AUDIENCE=<audience>`; set `AUTH_BYPASS=1` only for dev overrides.
  - Clerk auth (Surfer): `NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY`, `CLERK_SECRET_KEY`, optionally `NEXT_PUBLIC_CLERK_MANAGER_TOKEN_TEMPLATE`.
 - CLI auth: `CLERK_MOCK=1 beach login` (or `cargo run -p beach login`) seeds a local profile for private beaches; `BEACH_MANAGER_REQUIRE_AUTH=1` forces bearer tokens when working against custom hosts like `localhost`.

2) Manager
- `cargo run -p beach-manager`
- Health: `GET /healthz`
- Metrics: `GET /metrics` (Prometheus text; includes `actions_queue_depth`, `actions_queue_pending`, `action_latency_ms`)

3) Beach Road
- Local service for session registration and verification. Start via compose (`beach-road`) or run `cargo run -p beach-road` with `REDIS_URL`.
- Key endpoints used by Manager/UI:
  - `POST /sessions` (register; returns `join_code`)
  - `POST /sessions/:id/verify-code`
  - `GET /me/sessions?status=active`
  - `POST /sessions/:id/join-manager` (no-op nudge)

4) Seed a beach (one-time)
- Create org/account/private_beach rows (use psql) and keep the beach UUID handy. See docs/private-beach/roadmap.md for context.

5) Register sessions
- `POST /sessions/register` with `private_beach_id = <beach-uuid>`, unique `session_id` GUIDs, and `harness_type` (e.g., `terminal_shim`).
- Optional: `POST /sessions/:id/health` and `POST /sessions/:id/state` with small payloads to light up the UI.

6) Surfer (Next.js)
- `cd apps/private-beach && npm install && npm run dev -- -p 3001`
- Open `http://localhost:3001`, sign in with Clerk, and select a private beach. Manager URL defaults to `http://localhost:8080` and can be overridden in Settings.
- Live controls: Acquire/Release controller, Emergency Stop; live terminal tiles stream over WebRTC and drawers poll REST for controller events (Clerk session required).

## Ops & Monitoring
- **Viewer metrics** — Monitor `manager_viewer_connected`, `manager_viewer_latency_ms`, `manager_viewer_reconnects_total`, `manager_viewer_keepalive_sent_total`, `manager_viewer_keepalive_failures_total`, `manager_viewer_idle_warnings_total`, and `manager_viewer_idle_recoveries_total`. Wire alerts for sustained reconnect loops or keepalive failures.
- **TURN-only verification** — Set `BEACH_WEBRTC_DISABLE_STUN=1` on both Manager and hosts to force TURN. Confirm the viewer connects, keepalive metrics stay flat, and latency badge reflects the expected TURN hop. Use this before capacity tests.

## Configuration Notes
- Manager reads `BEACH_GATE_URL`; docker-compose now defaults it to `http://beach-gate:4133` for local networking. Override with your production Gate hostname and provide `BEACH_GATE_VIEWER_TOKEN` (plus the matching Gate/Road secrets) to enable viewer tokens end-to-end.

## End‑to‑End: Onboarding (Attach) — How to Test

Local (docker compose):
- By Code
  1. Register a Road session: `curl -sS -X POST http://localhost:4132/sessions -H 'content-type: application/json' -H 'x-account-id: auth-bypass' -d '{"session_id":"<uuid>"}'` and note `join_code`.
  2. UI: open a beach, click Add → By Code, paste the session ID + code. Or run `node temp/test-attach-by-code.mjs` with MANAGER_URL/ROAD_URL.
- My Sessions
  1. Register 1–2 sessions on Road with the same `x-account-id` header.
  2. UI: Add → My Sessions, select and Attach. Or run `node temp/test-attach-owned.mjs`.
- Launch New
  - UI shows a copyable command `beach run --private-beach <beach-id>`. CLI direct-registration is a follow-up; use By Code/My Sessions today.

Against api.beach.sh:
- Ensure Manager is deployed with the new routes and points to Road: `BEACH_ROAD_URL=https://api.beach.sh` (default now).
- Then attach by code with a real session created against `api.beach.sh`.

## What’s Built (Manager)
- SQLx-backed schema + migrations: `session`, `controller_lease`, `session_runtime`, `controller_event` (+ indexes, enums). RLS policies applied; per-request GUC `beach.private_beach_id` is set in transactions.
- Redis Streams for action queues with consumer groups; TTL caches for health/state; transparent fallback to in-memory for tests.
- SSE endpoint (legacy, retained for now): `GET /sessions/:id/state/stream` emits `state` events. The controller-event SSE was removed; drawers poll REST instead.
- Viewer credentials: `GET /private-beaches/:id/sessions/:sid/viewer-credential` prefers Gate-signed viewer tokens, but falls back to passcodes when the viewer token service is disabled (common in local dev).
- REST + MCP (JSON-RPC) covering session registration, listing, controller lease, queue/ack, health/state. MCP subscribe methods return `sse_url` helpers.
- Metrics: queue depth (`actions_queue_depth`), lag (`actions_queue_pending`), `action_latency_ms` histogram, Redis availability.
- Event audit enriched with principals (controller/issuer IDs) and filters in `GET /sessions/:id/controller-events?event_type=&since_ms=&limit=`.
- Session Onboarding endpoints:
  - `POST /private-beaches/:id/sessions/attach-by-code` → `{ ok, attach_method: "code", session }`
  - `POST /private-beaches/:id/sessions/attach` → `{ attached, duplicates }`
  - `POST /private-beaches/:id/harness-bridge-token` → `{ token, expires_at_ms, audience }` (dev stub)

### Fast‑Path (WebRTC) — Manager side
- Answerer endpoints:
  - `POST /fastpath/sessions/:session_id/webrtc/offer` → returns SDP answer
  - `POST /fastpath/sessions/:session_id/webrtc/ice` → add remote ICE
  - `GET /fastpath/sessions/:session_id/webrtc/ice` → list local ICE
- Data channel labels: `mgr-actions` (ordered), `mgr-acks` (ordered), `mgr-state` (unordered).
- Routing: `queue_actions` tries fast‑path first via `send_actions_over_fast_path`; on success logs metrics + events and returns. Otherwise falls back to Redis.
- Status: send path is live; receive paths (reading `mgr-acks` and `mgr-state`) are planned (see TODOs).

## What’s Built (Surfer)
- Sessions view listing session metadata + health with live WebRTC previews (shared `BeachTerminal`).
- Queue column shows `depth / lag` from Prometheus-backed state.
- Per‑session controls: Acquire/Release + Emergency Stop, gated by Clerk-issued Manager tokens.
- Lease countdown shown when controller is active.
- Settings allows overriding Manager/Road URLs; Clerk user tokens are fetched transparently for Manager REST requests and viewer credential fetches.
- Add Session modal:
  - Tabs: By Code (verifies with Road via Manager), My Sessions (lists from Road, bulk attach), Launch New (copyable CLI).

## Handoff TODOs (Ordered)
0. Controller drag & drop MVP (priority demo track):
   - Backend: add controller pairing schema and `POST/DELETE/GET /sessions/:id/controllers` (+ MCP equivalents) with basic prompt/cadence config.
   - Harness: subscribe to pairings, auto-manage leases, watch child over fast-path, honour prompt/update cadence, log fallback to HTTP.
   - Surfer: drag tile onto tile to assign control, modal for config, tile badges + pairing drawer, edit/remove actions.
   - Validation: extend fast-path smoke script with pairing steps, capture metrics/events for Grafana.
1. Surfer session tiles (next UX pass):
   - Persist drag-resize layout per beach (local storage for now, Manager layout API once it lands).
   - Provide a maximize/pop-out view that keeps the mini preview alive.
   - Add quick filters/search once the layout stabilises.
2. Harness fast‑path client (beach-buggy):
   - Dial manager endpoints (offer/answer/ICE) and open `mgr-actions`, `mgr-acks`, `mgr-state`.
   - Map ActionCommand/ActionAck/StateDiff over channels; enforce controller token.
   - Back-pressure + batching; fallback to Redis/HTTP if channel drops.
   - Kickoff June 19: manager transport hints now expose fast-path endpoints/channel labels; harness crate includes parsing scaffold (`crates/beach-buggy/src/fast_path.rs`).
   - June 19 update: `FastPathClient::connect` now performs SDP/ICE negotiation and surfaces an action broadcast + ack/state send helpers (integration with main harness loop still pending).
   - June 19 update: harness transport prefers fast-path channels for actions/acks/state with automatic HTTP fallback (`crates/beach-buggy/src/lib.rs`).
3. Manager fast‑path receive loops:
   - Listen on `mgr-acks` and call `ack_actions` with parsed acks (feed histograms/metrics as done for REST path).
   - Listen on `mgr-state` and call `record_state` (mirror to Redis + session_runtime).
   - Make both optional (feature flag) and observable (counters).
   - June 19 update: receive loops now run inside `FastPathSession::spawn_receivers`; acks/state payloads land in Redis/Postgres while HTTP remains fallback.
   - June 19 update: fast-path telemetry counters (`fastpath_actions_sent_total`, `fastpath_actions_fallback_total`, `fastpath_acks_received_total`, `fastpath_state_received_total`, `fastpath_channel_{closed,errors}_total`) now publish at `/metrics` for Grafana dashboards/alerts.
4. Surfer UX phase (see roadmap Phase 4):
   - Design system + components, IA, search/filtering, accessibility, performance budgets, polished session detail.
   - Auth via Beach Gate (OIDC); remove `access_token` query fallback.
   - Kickoff June 19: UX foundations brief captured in `docs/private-beach/ux-foundation-brief.md`.
5. Session onboarding hardening:
   - Replace dev bridge token with Beach Gate–minted scoped JWT; gate by beach/session.
   - Enforce ownership check against Beach Road for `attach` (currently trusting dev owner header).
   - Persist `session.attach_method` for audit (migration added; wire writes).
6. CI hardening:
   - Dockerized Postgres/Redis tests with `sqlx migrate run --check`.
   - Add an ignored integration test that mocks fast‑path (no WebRTC) to validate manager multiplex + acks/state.
7. Schema artifacts: generate drizzle-friendly SQL + enum maps for Surfer; publish alongside migrations.

## Manual Fast‑Path Test (for developers)
1. Start manager and register a session (as above).
2. Write a small Rust (or JS) peer to:
   - Create `RTCPeerConnection` with 3 channels: `mgr-actions` (ordered), `mgr-acks` (ordered), `mgr-state` (unordered).
   - Create local offer, send to `POST /fastpath/.../offer`, set remote description.
   - Exchange ICE via `POST/GET /fastpath/.../ice`.
   - On `mgr-actions` message, parse `{type:"action", payload}`; apply or simulate, then send ack on `mgr-acks`:
     `{id, status:"ok", applied_at:<now>, latency_ms:<ms>}`.
   - Periodically send a small `StateDiff` on `mgr-state`.
3. From another terminal, `POST /sessions/:id/actions` with a valid controller token; verify:
   - Peer receives actions over `mgr-actions`.
   - Manager metrics show `action_latency_ms` observations and pending gauges drop.
   - SSE `controller_event` includes `actions_queued`/`actions_acked` entries.

## Pointers
- Roadmap with dedicated UX phase: `docs/private-beach/roadmap.md`.
- Manager architecture + APIs: `docs/private-beach/beach-manager.md`.
- Harness spec: `docs/private-beach/beach-buggy-spec.md` (now mentions fast‑path channel labels).
 - Onboarding spec: `docs/private-beach/session-onboarding-plan.md` (implemented as described; see this STATUS for what shipped).

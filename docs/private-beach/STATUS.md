# Private Beach — Implementation Status (Handoff)

This document captures what’s built, how to run it locally, what’s left, and how to resume implementation quickly.

## TL;DR
- Manager (Rust) persists sessions, leases, and controller events in Postgres; commands/health/state use Redis Streams + TTL caches; SSE endpoints provide live updates; RLS is enforced via GUC.
- Surfer (Next.js) shows a sessions table with live events, queue depth/lag, lease countdown, and controls (acquire/release/stop). Auth bypass works with any token in local dev.
- Fast‑path (WebRTC) is scaffolded in the manager with answerer endpoints and routing to send actions over data channels when available. Harness‑side fast‑path client is next.

## Runbook

1) Dependencies
- `docker compose up postgres redis`
- Environment:
  - `DATABASE_URL=postgres://postgres:postgres@localhost:5432/beach_manager`
  - `REDIS_URL=redis://localhost:6379`
  - `AUTH_BYPASS=1` (implicit when Beach Gate vars are missing)

2) Manager
- `cargo run -p beach-manager`
- Health: `GET /healthz`
- Metrics: `GET /metrics` (Prometheus text; includes `actions_queue_depth`, `actions_queue_pending`, `action_latency_ms`)

3) Seed a beach (one-time)
- Create org/account/private_beach rows (use psql) and keep the beach UUID handy. See docs/private-beach/roadmap.md for context.

4) Register sessions
- `POST /sessions/register` with `private_beach_id = <beach-uuid>`, unique `session_id` GUIDs, and `harness_type` (e.g., `terminal_shim`).
- Optional: `POST /sessions/:id/health` and `POST /sessions/:id/state` with small payloads to light up the UI.

5) Surfer (Next.js)
- `cd apps/private-beach && npm install && npm run dev -- -p 3001`
- Open `http://localhost:3001`. Set Manager URL (`http://localhost:8080`), Private Beach ID, and a token (`test-token`). Click Refresh.
- Live controls: Acquire/Release controller, Emergency Stop; see events/state via SSE.

## What’s Built (Manager)
- SQLx-backed schema + migrations: `session`, `controller_lease`, `session_runtime`, `controller_event` (+ indexes, enums). RLS policies applied; per-request GUC `beach.private_beach_id` is set in transactions.
- Redis Streams for action queues with consumer groups; TTL caches for health/state; transparent fallback to in-memory for tests.
- SSE endpoints:
  - `GET /sessions/:id/state/stream` emits `state` events
  - `GET /sessions/:id/events/stream` emits `controller_event`/`state`/`health`
- REST + MCP (JSON-RPC) covering session registration, listing, controller lease, queue/ack, health/state. MCP subscribe methods return `sse_url` helpers.
- Metrics: queue depth (`actions_queue_depth`), lag (`actions_queue_pending`), `action_latency_ms` histogram, Redis availability.
- Event audit enriched with principals (controller/issuer IDs) and filters in `GET /sessions/:id/controller-events?event_type=&since_ms=&limit=`.

### Fast‑Path (WebRTC) — Manager side
- Answerer endpoints:
  - `POST /fastpath/sessions/:session_id/webrtc/offer` → returns SDP answer
  - `POST /fastpath/sessions/:session_id/webrtc/ice` → add remote ICE
  - `GET /fastpath/sessions/:session_id/webrtc/ice` → list local ICE
- Data channel labels: `mgr-actions` (ordered), `mgr-acks` (ordered), `mgr-state` (unordered).
- Routing: `queue_actions` tries fast‑path first via `send_actions_over_fast_path`; on success logs metrics + events and returns. Otherwise falls back to Redis.
- Status: send path is live; receive paths (reading `mgr-acks` and `mgr-state`) are planned (see TODOs).

## What’s Built (Surfer)
- Sessions view listing session metadata + health with live SSE feed.
- Queue column shows `depth / lag` from Prometheus-backed state.
- Per‑session controls: Acquire/Release + Emergency Stop.
- Lease countdown shown when controller is active.
- Configurable manager URL + token in UI; SSE supports `?access_token=` for local testing.

## Handoff TODOs (Ordered)
1. Harness fast‑path client (beach-buggy):
   - Dial manager endpoints (offer/answer/ICE) and open `mgr-actions`, `mgr-acks`, `mgr-state`.
   - Map ActionCommand/ActionAck/StateDiff over channels; enforce controller token.
   - Back-pressure + batching; fallback to Redis/HTTP if channel drops.
2. Manager fast‑path receive loops:
   - Listen on `mgr-acks` and call `ack_actions` with parsed acks (feed histograms/metrics as done for REST path).
   - Listen on `mgr-state` and call `record_state` (mirror to Redis + session_runtime).
   - Make both optional (feature flag) and observable (counters).
3. Surfer UX phase (see roadmap Phase 4):
   - Design system + components, IA, search/filtering, accessibility, performance budgets, polished session detail.
   - Auth via Beach Gate (OIDC); remove `access_token` query fallback.
4. CI hardening:
   - Dockerized Postgres/Redis tests with `sqlx migrate run --check`.
   - Add an ignored integration test that mocks fast‑path (no WebRTC) to validate manager multiplex + acks/state.
5. Schema artifacts: generate drizzle-friendly SQL + enum maps for Surfer; publish alongside migrations.

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


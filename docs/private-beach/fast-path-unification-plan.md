# Fast-Path Unification Plan

## Goal
Fast-path must be the default, first-class transport for controller + state traffic across **every** Private Beach component (agent harnesses, player hosts, and Beach Manager). Redis/HTTP becomes a bounded fallback path that only engages when fast-path is unavailable. This eliminates the controller queue wedging we see today when the host pauses HTTP but the manager is still sourcing controller actions from Redis.

## Scope
- Applies to all harnesses built on `crates/beach-buggy` (Rust, CLI agents, future apps).
- Applies to Beach Manager controller delivery (`apps/beach-manager`).
- Applies to CLI hosts (`apps/beach`) HTTP poller gating.
- No feature flags or staged rollout: we are greenfielding the canonical behavior.

## High-Level Architecture
1. **Harness fast-path (mgr-actions/mgr-acks/mgr-state)**
   - Every harness using `HttpTransport` automatically connects to the manager-specified fast-path endpoints (from `transport_hints.fast_path_webrtc`).
   - Each session gets a `FastPathSession` in the manager registry, keyed by session ID, as soon as `FastPathClient::connect` completes.

2. **Manager queueing semantics**
   - `AppState::queue_actions` sends controller actions exclusively over fast-path when a `FastPathSession` exists.
   - Redis/HTTP fallback is used only when there is no fast-path session or when fast-path send fails; fallback queues must remain bounded and self-healing.

3. **Host fast-path (controller forwarder)**
   - Hosts pause HTTP polling only after the manager confirms zero pending HTTP actions *and* transport status is `fast_path`.
   - If fast-path drops, hosts automatically resume HTTP polling.

4. **Acknowledgements + XPENDING hygiene**
   - Manager reclaims/acknowledges pending Redis entries whenever a consumer dies so `drain_actions_redis` never logs “non-empty stream” repeatedly.

## Detailed Tasks

### 1. Harness fast-path (crates/beach-buggy)
- [ ] `HttpTransport::new` should immediately parse `transport_hints.fast_path_webrtc` (the hints already exist) and schedule a connection attempt.
- [ ] `HttpTransport::register_session` and any call that refreshes credentials should refresh the fast-path endpoints (in case they change) and reconnect if necessary.
- [ ] `receive_actions` should prefer the fast-path broadcast channel before touching HTTP. If no fast-path connection exists, fall back to HTTP as today.
- [ ] Provide metrics/logs around fast-path connection state (connected, retrying, failed) so manager operators can correlate with controller queues.
- [ ] Ensure state push + ack loops send over fast-path channels whenever available (state on `mgr-state`, acks on `mgr-acks`).

### 2. Manager queue_actions / fallback
- [ ] In `AppState::queue_actions`, treat `FastPathSendOutcome::SessionMissing` / `ChannelMissing` as actionable: log once and return `ControllerCommandDropReason::FastPathNotReady` rather than enqueueing blindly, unless the caller opted into HTTP fallback.
- [ ] Introduce a per-session `transport_mode` flag (default `FastPath`) inside `SessionRecord` so manager knows whether fallbacks are expected. Pong players will flip this to `HttpFallback` until the CLI host gains harness parity.
- [ ] When enqueuing via Redis fallback, ensure XPENDING is drained when hosts disconnect (call `XACK` / `XDEL` for stale entries) so `drain_actions_redis` does not get stuck.
- [ ] When fast-path delivery succeeds, *never* enqueue the same actions to Redis. Fast-path should be single-source-of-truth in steady state, and Redis backlog should stay near zero.

### 3. Host HTTP poll gating (apps/beach/src/server/terminal/host.rs)
- [ ] Replace the simple “fast-path channel ready -> pause HTTP” heuristic with: pause only when `pending_actions_depth == 0` *and* manager transport status is `fast_path`.
- [ ] Keep polling HTTP (with backoff) if manager reports fast-path not ready, even if the local `mgr-actions` channel exists.
- [ ] Resume HTTP immediately when manager demotes the session to HTTP fallback.

### 4. Controller forwarder + acknowledgements (apps/beach-manager/src/controller/...)
- [ ] Ensure fast-path forwarder always calls `ack_actions` once acks arrive via `mgr-acks`. This removes the need for HTTP poller to ack.
- [ ] When HTTP poller drains actions (fallback), ensure acknowledgements clear XPENDING entries and log when the queue returns to zero.

### 5. Instrumentation + validation
- [ ] Metrics: expose per-session transport status (`fast_path` vs `http_fallback`), fast-path connection health, and Redis queue depth gauges to Grafana.
- [ ] Integration tests: add a harness-level test that mocks fast-path endpoints and ensures `queue_actions` uses fast-path (zero Redis writes). Add another test covering HTTP fallback when fast-path connection is absent.

## Deliverables
1. Updated harness runtime (`crates/beach-buggy`) that establishes and prefers fast-path for all sessions.
2. Manager changes (`apps/beach-manager`) that treat fast-path as the primary path and keep Redis fallback bounded.
3. Host CLI updates to keep HTTP polling active until managers says it’s safe to stop.
4. Documentation updates summarizing the new contract (this document + README references).

## Execution Notes
- No feature flags; change the default behavior.
- Pong agent will move to the Rust harness (or a Python equivalent of `FastPathClient`) later. For now, document that Python agents must adopt the same fast-path contract or stay on HTTP fallback explicitly.
- Keep all changes in sync so we don’t regress into a half-fast-path/half-HTTP state.

## Tracking
Use this document to track progress. When a task is complete, note the PR/commit and any follow-up work required.

- [x] Harness + CLI transport updates (PR TBD): `HttpTransport` can now ingest fast-path hints immediately, logs connection status, and the CLI idle snapshot/health transports apply those hints so state/ack loops ride the mgr-state/mgr-acks channels by default.
- [x] Manager delivery semantics (PR TBD): `AppState::queue_actions` prefers fast-path based on the new per-session `transport_mode`, rejects commands with `FastPathNotReady` when sessions are marked fast-path-only but the channel is missing, and Redis fallback stays bounded via XPENDING reclamation + metrics (`redis_pending_reclaimed_total`).
- [x] Host gating (PR TBD): `/actions/pending` now returns `fast_path_ready` + `transport`, and the CLI host only pauses HTTP polling when the manager reports fast-path ready with zero pending actions (resuming automatically if the manager falls back).

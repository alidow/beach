# Beach Manager Rewrite (apps/beach-manager-rewrite)

Goal: stand up a parallel, cleanly factored manager that joins every host as a standard Beach client on the unified WebRTC transport (single channel; no legacy fast-path), with first-class traceability and a flip-able toggle between legacy and rewrite.

## Requirements & Constraints
- Join each host as a normal Beach client via Beach Road signaling; send/receive WebRTC extension frames (namespace `manager`, kinds `action|ack|state|health`) that Beach Buggy already understands (`mgr-actions` label et al.).
- Rewrite must not spin legacy extra peers or secondary RTCPeerConnections; unified transport only with the labels above.
- Handle many hosts per instance (target ≤50) and support horizontal scale: deterministic assignment of host_session_id → manager instance; controller relationships must be tracked even when hosts are on different managers.
- No single file >2k lines; prefer small modules by domain (transport, registry, auth, controller, metrics, persistence, api).
- Zero-trust posture: Clerk/Beach Gate JWT verification; scoped capabilities enforced on every handler.
- Observability-first: every handshake/attach/transport change is traceable with correlation ids and metrics; logs must call out the failing phase and peer/session ids.
- Unified pub/sub: the WebRTC extension channel acts as a message bus; any crate (manager, beach-buggy, future agents) can subscribe to topics/labels and publish back without bespoke side channels.
- Host is transport-only: `apps/beach/` stays a cache sync + pub/sub + PTY layer; it has no awareness of private beach concepts. Manager/bus messages provide all beach-specific context/tokens to the harness.

## Connectivity Risk Map & Trace Hooks
- Signaling ordering: attach must precede offer/answer; log `attach_phase={pending,ok}` with `host_session_id`, `peer_session_id`.
- Auth/JWKS drift: log issuer/aud/kid on failures; metric for `auth_failure_total`.
- ICE/TURN mismatch: emit resolved `ice_servers`, `public_ip/host`, and candidate policy; warn if TURN/ICE unset or if private IP is advertised while manager is in Docker.
- SDP/channel labels: assert labels and namespaces match expected (`manager`); reject/trace on mismatch.
- Data channel state: trace `dc_state_change` (label, new_state, transport_id, seq); metric gauges for RTC ready time and reconnect count.
- Backpressure/queue: log queue depth per host and drop reasons; metrics for `actions_inflight`, `acks_pending`.
- Extension routing: trace every ingress/egress extension frame at debug with size + lane; warn on unknown namespace/kind.
- HTTP fallback: only after explicit timeout; trace downgrade with reason and transport stats.
- Persisted mapping: log deterministic manager assignment decisions and when ownership moves.

## Architecture (modular)
- `config/`: env + feature toggle (`BEACH_MANAGER_IMPL=legacy|rewrite` shared across services/tests).
- `telemetry/`: tracing subscriber, structured log fields (host_session_id, peer_session_id, manager_instance_id, transport_id, controller_token suffix), metrics registry.
- `auth/`: Gate/Clerk JWT verification + scope checker; JWKS refresh; bypass only in dev.
- `transport/`: Beach client shim that joins via Beach Road, negotiates unified transport, subscribes to `manager` namespace, exposes lanes (ordered control, unordered state) and health.
- `registry/`: host attachment records, manager assignment map, controller leases (in Postgres), runtime/health cache (Redis).
- `controller/`: action enqueue/ack/state ingestion over extensions; Redis Streams + backpressure.
- `api/`: Axum routes for health, attach status, debug traces, and (later) Surfer/agent APIs mirrored from legacy.
- `assign/`: manager membership + rendezvous/consistent hashing to map hosts to managers; heartbeat + capacity (max 50) enforcement.
- `bus/`: unified message-bus facade over the WebRTC extension channel; each participant exposes explicit `subscriber.rs`/`publisher.rs` (or `subscriber/mod.rs`, `publisher/mod.rs`) modules so it’s clear what topics are consumed/produced.
- `bus/`: unified message-bus facade over the WebRTC extension channel so beach-buggy (and future clients) subscribe/publish by namespace/topic instead of bespoke handlers; keeps backward compatibility by emitting legacy hints when needed and aliasing legacy topics.

## Milestones (bite-sized, trackable)
- [ ] M0: Scaffold crate `apps/beach-manager-rewrite` with Axum/Tokio, config, telemetry, health endpoints; lint/tests wired.
- [ ] M1: Auth layer with Gate/Clerk validation + bypass flag; per-request correlation fields.
- [ ] M2: Beach client shim that performs attach (host_session_id → peer_session_id) and negotiates unified transport via Beach Road; logs all phases.
- [ ] M3: Extension router: subscribe/send `manager` namespace; lane selection + metrics; unit tests for routing/unknown labels.
- [ ] M3b: Connectivity smoke ready: bring up the dedicated smoke stack and pass the 60s WebRTC/cache sync smoke (host ↔ manager-rewrite) on alternate ports.
- [ ] M3c: Message bus shim: wrap the unified channel in a topic-based bus (namespaces/topics) with adapter for beach-buggy and manager-as-just-a-subscriber/publisher; emit compatibility hints (`extensions.namespaces=["manager"]`) so hosts opt in uniformly.
- [ ] M3d: Host bus adoption (partial): remove controller auto-attach gating in `apps/beach`, always stand up the bus on unified transport, and drop legacy fastpath peers.
- [ ] M3e: Buggy controller over bus: beach-buggy consumes `controller/input` and publishes `controller/ack|state|health` on the bus; host no longer handles controller logic directly.
- [ ] M3f: Manager controller over bus: manager publishes actions and consumes acks via the bus, no HTTP/controller-specific channels.
- [ ] M3g: Subscriber/publisher clarity: each participant (host/beach-buggy/manager) has explicit `subscriber.rs`/`publisher.rs` modules listing topics and handling, to make the bus wiring easy to audit.
- [ ] M4a: Extension→queue wiring: action/ack/state extensions flow into Redis Streams with backpressure limits; tests cover drop/queue/ack paths in-memory + Redis.
- [ ] M4b: Persistence: controller leases/events persisted to Postgres with RLS; idempotent writes and retries; Postgres-backed tests.
- [ ] M5: Manager assignment service: Postgres/Redis-backed membership table, heartbeat, rendezvous hashing for host → manager; refuse assignment if capacity exceeded; tests for reassignment; early smoke that runs attach + action across two managers.
- [ ] M6: Surfacing APIs (REST/MCP parity subset): attach-by-code passthrough to Road, register session, controller lease acquire/release; SSE deprecated in favor of WebRTC-only; tests for auth + RLS.
- [ ] M7: E2E smokes: docker-compose stack with rewrite manager; `pong-stack.sh` variant hitting rewrite; CI job to run webrtc tester against rewrite.
- [ ] M8: Cleanup: flip default via env, document migration, delete unused code paths post-acceptance.
- [ ] M9: Hot failover prep: add secondary manager attachment and cache warmback for seamless host failover; tests that drop primary and verify reconnect to secondary without losing controller context.
- [ ] M9b: Controller-path smoke: in the smoke stack, run a controller host/agent that drives a target host (e.g., Pong player) through manager-rewrite; assert actions flow and state updates reflect control without HTTP fallback.

## Docker Compose & Toggle Plan
- Add service `beach-manager-rewrite` to `docker-compose.yml` (example):
  ```yaml
  beach-manager-rewrite:
    container_name: beach-manager-rewrite
    build:
      context: .
      dockerfile: Dockerfile # or reuse cargo-run entrypoint
    command: exec /app/scripts/docker/beach-manager-entry.sh cargo run -p beach-manager-rewrite
    ports: ["8081:8081"]
    env_file: .env
    environment:
      BEACH_MANAGER_INSTANCE_ID: beach-manager-rewrite-1 # per-instance identity for heartbeat/assignment
      BEACH_MANAGER_IMPL: rewrite                      # shared toggle read by scripts/services
      BEACH_MANAGER_CAPACITY: 50
      PRIVATE_BEACH_MANAGER_URL: http://beach-manager-rewrite:8081
    volumes:
      - ./logs/beach-manager-rewrite:/var/log/beach-manager-rewrite
      - cargo-registry-beach-manager-rewrite:/usr/local/cargo/registry
  ```
- Central toggle: `.env` key `BEACH_MANAGER_IMPL=legacy|rewrite`. Downstream services/scripts read it to choose manager URL:
  - `MANAGER_URL` helper var exported from `.env` / launch scripts based on the toggle (`http://beach-manager:8080` vs `http://beach-manager-rewrite:8081`).
  - `PRIVATE_BEACH_MANAGER_URL=${MANAGER_URL}` consumed by Beach Road/Surfer/tests; ensure launch scripts export both so they propagate.
- `pong-stack.sh` integration:
  - Accept `PONG_MANAGER_IMPL` or reuse `BEACH_MANAGER_IMPL`.
  - Set `PONG_DOCKER_SERVICE=${impl == rewrite ? "beach-manager-rewrite" : "beach-manager"}` and `PRIVATE_BEACH_MANAGER_URL` to service URL inside docker network.
  - When starting the stack, ensure `docker compose up beach-manager-rewrite beach-road ...` includes rewrite service; players/agent inside docker resolve `beach-manager-rewrite:8081`. For hosts outside docker, use the published port/hostname (e.g., `http://localhost:8081` or `http://api.beach.dev:8081` with /etc/hosts).

## Horizontal Scaling Strategy
- Manager membership: each instance registers heartbeat in Redis/DB with `manager_instance_id`, `capacity`, `current_load`, `endpoints`; heartbeat every 5s with TTL 15s.
- Assignment: rendezvous hashing on `host_session_id` across live instances; if chosen manager at capacity, pick next hash entry; record decision in Postgres (`manager_assignment` table) so controllers know where to connect and auditors can trace moves.
- Schema sketch: `manager_instance(id, endpoint, capacity, last_heartbeat_at, load)`, `manager_assignment(host_session_id, manager_instance_id, assigned_at, reassigned_from?, reason)`.
- Controller awareness: leases stored in Postgres; action fan-out uses assignment map to route to the correct manager (if cross-manager, publish over Redis stream per host manager).
- Failure handling: on two missed heartbeats (~10s), rehash and reissue attach hints; log reassignment with old/new manager ids and affected hosts.

### Hot Failover Path (target for M9)
- Dual attachments: hosts negotiate a primary transport and optionally a standby transport to a secondary manager (from rendezvous hashing second choice) but keep standby dormant until failover.
- Warm cache: shared Redis/state snapshots allow secondary to serve state; periodic state/health replication ensures secondary is warm (or secondary subscribes to state diffs via shared stream).
- Failover trigger: on primary loss (missed heartbeats, transport down), host initiates attach/offer to secondary and reuses controller token; manager verifies lease and resumes action/ack flow.
- Observability: log `failover_initiated` with old/new manager ids and host session; metric `failover_total`, gauge for standby-ready hosts.
- Testing: inject primary drop in docker-compose (stop container or block port), assert host reconnects to secondary within SLO and controller actions/acks continue. Add a smoke script:
  - Start two managers (`beach-manager-rewrite-1` primary, `-2` secondary), Road, Redis.
  - Launch a host and controller; record manager assignment and active transport id.
  - Send a controller action; assert ack received over primary.
  - Kill or pause primary container; wait for standby promote; resend action; assert ack comes via secondary and state cache remains consistent.
  - Emit metrics/log snapshot for failover latency.

## Observability Defaults
- Log fields on every span: `manager_instance_id`, `host_session_id`, `peer_session_id`, `controller_token_suffix`, `transport_id`, `attach_attempt`, `phase` (attach|offer|answer|ice|dc_open|ready|downgrade|reconnect), `transport_mode` (webrtc|http_fallback).
- Metrics: `transport_rtc_ready_seconds` (SLO target <5s p95), `transport_reconnect_total`, `actions_enqueued_total`, `acks_pending`, `extension_sent_bytes_total`, `extension_dropped_total`, `auth_failure_total`, `assignment_rebalance_total`, `attach_failure_total`, `http_fallback_total`.
- Debug endpoints: `/debug/assignments`, `/debug/transports/:id` (summaries only, not payloads).
- Crash safety: panic hook prints current attach/transport state and recent errors.

## File Size Guardrails
- Enforce module boundaries; CI lint checks fail if any file exceeds ~1.8k lines (e.g., `scripts/check-lines.sh` guarding `apps/beach-manager-rewrite/src/**/*.rs`).
- Preferred layout: one module per domain above; tests colocated in `tests/` where possible.

## Early Connectivity Testing (before full feature set)
1) Scaffold (M0) + auth (M1) with health endpoints returning instance id + impl.  
2) Implement attach + transport negotiation (M2) and add a smoke that spins a host (inside docker) and asserts RTC ready within SLO, cache sync (state/health heartbeat) visible in logs/metrics.  
3) Extension router (M3) minimal: send/receive `manager` namespace messages; write a tiny harness hook to log received manager control frames and assert they’re delivered.  
4) Only after steps 1–3 are green, proceed to queues/persistence (M4).  

### Dedicated connectivity smoke stack
- Maintain a standalone smoke stack under `apps/beach-manager-rewrite/tests/smoke/` with its own `docker-compose.smoke.yml` mirroring the root topology (Road, Redis, two managers optional) but on alternate ports (e.g., manager 18081, road 14132) so it can run in parallel with the main stack.
- Automation: add a scripted smoke (`apps/beach-manager-rewrite/tests/smoke/run.sh` or a `cargo test -- --ignored connectivity_smoke` that shells out) that:
  - Brings up the smoke stack and waits for health on the rewrite manager.
  - Launches a host inside the smoke network; negotiates WebRTC with the rewrite manager and asserts RTC ready.
  - Uses IPC to mutate the host cache (e.g., invoke beach-buggy API or a small helper CLI inside the host container to push a state diff) and polls the manager-side beach client (IPC) to confirm the cache/state reflects the change within 1 minute.
  - Runs for at least 60s, asserting transport stays connected and caches stay in sync; fails on disconnect/reconnect storms or missing state updates.
  - Tears down the smoke stack cleanly.

#### Controller-path smoke (later milestone)
- Extend the smoke stack to launch two hosts: one designated controller (agent) and one target (player).
- Acquire a controller lease via manager-rewrite, send control actions from the controller, and assert the target host applies them (state/ack observed) without HTTP fallback.
- Suitable to run at M9b once controller pipeline + unified transport are stable.

### Message bus model (compatibility + clarity)
- Treat the unified WebRTC extension channel as a message bus: all traffic is `{namespace, topic, payload}`; beach-buggy subscribes to its topics (e.g., `manager:action`, `manager:ack`, `manager:state`, `manager:health`) and can publish too. Manager is just another subscriber/publisher on the same bus.
- Backward compatibility: advertise `extensions.namespaces=["manager"]` only; if any legacy host still expects the old label, provide a shim to map it client-side and then delete once all hosts are updated.
- Enforcement: namespace ACLs ensure viewers don’t receive controller topics; metrics/logs per topic for observability.
- Intent: a simple pub/sub model so any crate (manager, buggy, agents) can attach listeners and publish without bespoke channels; unified channel stays the single transport.
- Beach association handshake: when a host is linked to a private beach, manager publishes a bus message (manager namespace/topic) to that host’s harness with everything needed to interact (manager/beach URL, bridge token/lease info, attach code, idle snapshot hints). Host does not bake in beach knowledge; it just consumes the message and uses the provided token on subsequent calls.
- Incremental rollout plan:
  - Phase 1: host always stands up the bus once unified transport is ready; controller auto-attach logic removed.
  - Phase 2: buggy handles controller input/ack/state/health solely via bus topics; host delegates.
  - Phase 3: manager publishes actions and listens for acks on bus; legacy controller channels removed.
  - Phase 4: controller-path smoke (M9b) runs entirely on bus; remove legacy shims.
- Conventions: bus-facing modules live in `bus/subscriber.rs` and `bus/publisher.rs` (or `subscriber/mod.rs`, `publisher/mod.rs`) per participant so topic handlers are easy to find.

## Next Steps to Start
1) Create crate skeleton (`cargo new apps/beach-manager-rewrite --bin`), add to workspace.  
2) Add config/telemetry/auth modules (M0–M1) with tests and file-size lint.  
3) Wire Beach client shim + extension router (M2–M3) and add unit tests + early smoke above.  
4) Add compose service + `.env` toggle; run `docker compose up beach-manager-rewrite beach-road` and verify health + attach smoke.  
5) Run `env BEACH_MANAGER_IMPL=rewrite ./apps/private-beach/demo/pong/tools/pong-stack.sh start` and confirm hosts connect to rewrite inside docker network.  
6) Iterate through milestones with checkboxes above and update this doc as milestones complete. 

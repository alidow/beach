# Beach Manager Control Plane

## Mission
Beach Manager is the zero-trust control plane that keeps Private Beach cohesive. It is the single source of truth for:
- Session registration, controller leases, and automation assignments.
- Policy enforcement (role-based access, rate limiting, network posture).
- Agent onboarding workflow (prompt packs, MCP bridge wiring, capability gating).
- Audit logging and telemetry routed to billing, alerting, and analytics systems.

## Terminology
- **Manager (service):** This Rust control plane (`apps/beach-manager`) that arbitrates every action inside a private beach—authenticates tokens, owns the session/lease registry, fans out state, and records audits.
- **Controller (session):** A Beach session plus its Beach Buggy harness that currently holds a controller lease for another session. Controllers call orchestration APIs (`queue_action`, `subscribe_state`, etc.) while still behaving like ordinary sessions when they are not leased.
- **Harness:** The per-session Beach Buggy sidecar that streams diffs and consumes actions. Every controller is backed by a harness, but most harnesses are *not* controllers; they simply publish state until a controller lease is granted.

## High-Level Architecture
1. **API Layer (Axum)**
   - REST + WebSocket endpoints for the Private Beach Surfer app.
   - JSON-RPC/MCP endpoints for harnesses, agents, and CLI tools.
   - Beach Gate JWT verification middleware with capability scopes (e.g., `pb:session.read`, `pb:control.write`).
2. **Domain Services**
   - `SessionService`: maintains session records, harness metadata, P2P negotiation hints, and controller leases.
   - `AutomationService`: handles agent registration, prompt provisioning, MCP bridge catalogs, and lease scheduling.
   - `PolicyService`: evaluates RLS-style predicates against org/beach membership, group roles, and custom policies.
   - `AuditService`: writes append-only controller events, state access logs, and automation outcomes to Postgres + stream bus.
3. **Data Stores**
   - Postgres (per `docs/private-beach/data-model.md`) for durable org/beach state, memberships, automation assignments, controller events, share links.
   - Redis Cluster for state cache, message fan-out, and transient command queues; sharded by private beach.
   - S3-compatible object store for persistent files and archival snapshots.
4. **Integration Points**
   - Harness registration via `crates/beach-buggy` (terminal shim, Cabana adapter, future widgets).
  - Private Beach Surfer UI for human workflows.
   - Agent SDKs (`crates/manager-sdk`) for bots or scripting.
   - Billing/analytics sinks (webhooks or internal pipelines).

## Key Flows
### Session Registration
1. Harness authenticates with Beach Gate + manager-signed token.
2. Calls `POST /sessions/register` (or MCP equivalent) with harness metadata and capabilities.
3. Manager stores session row, returns controller lease options, P2P hints, and state cache endpoints.
4. Harness begins streaming diffs through Redis/WebRTC per negotiated transport.

### Session Onboarding & Attach
1. Add by Code (public session claim)
   - UI posts `session_id` + `code` to Manager: `POST /private-beaches/:id/sessions/attach-by-code`.
   - Manager verifies with Beach Road `POST /sessions/:id/verify-code` (proof of control, owner identity, harness hint).
   - Manager mints a short‑lived bridge token (scopes: `pb:sessions.register pb:harness.publish`) via Beach Gate and instructs Road to nudge the harness.
   - Harness connects to Manager and calls `POST /sessions/register` using the bridge token; Manager persists mapping (`session.origin_session_id`, beach ID) and emits events.
2. Attach My Sessions (owned)
   - UI calls Beach Road `GET /me/sessions?status=active` with user JWT; user selects one or more.
   - UI posts to Manager: `POST /private-beaches/:id/sessions/attach` with `{ origin_session_ids: [] }`.
   - Manager validates ownership via Beach Road, persists mappings, and (if not connected) mints a bridge token and asks Road to `POST /sessions/:id/join-manager`.
3. Launch New (CLI)
   - UI presents copyable `beach run --private-beach <beach-id>` (and Cabana variant). CLI uses Beach Gate user token to mint a scoped token and registers directly to Manager at startup.

APIs (Manager additions)
- `POST /private-beaches/:private_beach_id/sessions/attach-by-code` → { ok, attach_method, mapping }
- `POST /private-beaches/:private_beach_id/sessions/attach` → { attached: number, duplicates: number }
- `POST /private-beaches/:private_beach_id/harness-bridge-token` → { token, expires_at_ms, audience }

APIs (Beach Road additions)
- `POST /sessions/:origin_session_id/verify-code` → { verified, owner_account_id, harness_hint }
- `GET /me/sessions?status=active` → list of active sessions owned by caller
- `POST /sessions/:origin_session_id/join-manager` { manager_url, bridge_token }

### Controller Lease & Action Pipeline
1. Client/agent acquires lease via `POST /sessions/:id/controller/lease`.
2. Manager validates scopes, updates Postgres + emits `controller_event`.
3. Actions enqueued via queue API (REST or MCP). Manager routes to harness through Redis or direct WebRTC data channel.
4. Harness ACKs through `beach-buggy` primitives; manager records latency + status for observability and alerts.

### Agent Onboarding
1. User designates a session as a manager via Private Beach UI; selects template (e.g., “Pong Coordinator”, “Ops Runbook”).
2. Manager verifies the session’s harness type and entitlements.
3. Manager generates prompt pack, configures MCP bridge map (e.g., `beach_state`, `beach_action`, custom domain tools).
4. Manager issues scoped token enabling the agent to call orchestration APIs; prompts and capabilities delivered through secure channel.
5. Monitoring hooks watch agent activity; anomalies can auto-revoke lease.

## Security Posture
- **Zero Trust:** assume infra compromise; require signed JWTs + optional mutual TLS between harness and manager.
- **Row-Level Security:** enforce per-private-beach access at the database layer in addition to application checks.
- **Attach Authorization:** attaching a session requires beach admin/owner role and either proof‑of‑control (code) or ownership (Road lookup). Bridge tokens are short‑lived and scoped to a single session and beach.
- **Rate Limits:** dynamic budgets per session and controller; throttle automation loops that risk flooding transports.
- **Audit Completeness:** controller events, state reads, and agent command results must be stored before responses are acknowledged.
- **Secrets Handling:** manager uses short-lived tokens (≤5 min) for agent/harness access; refresh tokens stored encrypted.

## Observability
- OpenTelemetry tracing spans across API layer, Redis command enqueue, harness ACK path.
- Metrics: command latency histograms, queue depth, harness heartbeat freshness, lease churn.
- Structured logs with correlation IDs (private beach, session, controller token) for incident response.

## Fast‑Path Transport Status
- Manager exposes WebRTC answerer endpoints for fast‑path negotiation:
  - `POST /fastpath/sessions/:session_id/webrtc/offer` → returns SDP answer
  - `POST /fastpath/sessions/:session_id/webrtc/ice` → add remote ICE
  - `GET /fastpath/sessions/:session_id/webrtc/ice` → list local ICE candidates
- Data channel labels: `mgr-actions` (ordered, reliable), `mgr-acks` (ordered, reliable), `mgr-state` (unordered).
- Routing: `queue_actions` prefers fast‑path; falls back to Redis if unavailable. Metrics/events are recorded identically.
- Next steps: manager listeners for `mgr-acks` → `ack_actions`, and `mgr-state` → `record_state`; feature‑flag and add per‑lane counters.

## Fast-Path Transport (WebRTC Data Channel)
- Goal: low-latency manager↔harness path for actions/state alongside Redis/HTTP.
- Approach: reuse Beach’s existing WebRTC stack and signaling model instead of embedding a separate WebRTC engine into the manager.
  - The harness will provision additional RTCDataChannels labelled `mgr-actions` (ordered, reliable) and `mgr-state` (unordered, low-latency) when a “manager peer” joins.
  - The manager participates as a viewer-level peer via Beach Road signaling (same offer/answer/ICE envelope), authenticated with a scoped token and gated by the controller lease.
  - Frames over `mgr-actions` map to the same `ActionCommand`/`ActionAck` envelopes used today; `mgr-state` carries `StateDiff`.
- Fallbacks: if signaling/ICE fails, the system automatically uses Redis Streams + HTTP POSTs; both paths are first-class.
- Phasing:
  1. Add signaling primitives and channel labels across host/harness (no UI impact).
  2. Teach `AppState` to multiplex action routes (Redis vs. WebRTC) and record per-lane metrics.
  3. Harden auth (capability-scoped JWT + RLS correlation), alerting, and back-pressure.
  4. Promote as default where TURN is available; retain Redis path for compatibility/debug.


## API Contracts

### REST (OpenAPI Draft v0.1)
| Endpoint | Method | Request | Response | Notes |
| --- | --- | --- | --- | --- |
| `/sessions/register` | POST | `{ session_id, private_beach_id, harness_type, capabilities[], location_hint?, metadata? }` | `{ session_id, harness_id, controller_token, lease_ttl_ms, state_cache_url, transport_hints }` | Called by Beach Buggy harness during attach. Requires `pb:sessions.register`. |
| `/sessions/{session_id}` | PATCH | `{ title?, display_order?, tags?, automation_assignment? }` | Updated session resource | Metadata tweaks from UI. |
| `/sessions/{session_id}/controller/lease` | POST | `{ requesting_account_id, ttl_ms?, reason? }` | `{ controller_token, expires_at }` | Acquire controller lease; emits `controller_event`. |
| `/sessions/{session_id}/controller/lease` | DELETE | `{ controller_token }` | `{ released: true }` | Release lease; revokes outstanding commands. |
| `/sessions/{session_id}/actions` | POST | `{ controller_token, actions[{ id, type, payload, priority?, dedupe_key?, expires_at? }] }` | `{ accepted_ids[], rejected[{ id, code, message }] }` | Queue commands for harness execution. |
| `/private-beaches/{id}/sessions` | GET | Query: `include_health?, include_tags?` | `[SessionSummary]` | Used by Private Beach web dashboard. |
| `/sessions/{session_id}/controller-events` | GET | `{ cursor?, limit? }` | `[ControllerEvent]` | Audit/history feed. |
| `/agents/onboard` | POST | `{ session_id, template_id, scoped_roles[], options }` | `{ agent_token, prompt_pack, mcp_bridges[] }` | Bootstraps automation manager sessions. |
| `/healthz`, `/readyz` | GET | – | `{ status: "ok" }` | Liveness/readiness probes. |

All REST endpoints published via OpenAPI at `/openapi.json` (versioned per release).

### MCP (JSON-RPC over WebSocket)
| Method | Params | Result | Notes |
| --- | --- | --- | --- |
| `private_beach.register_session` | `{ session_id, private_beach_id, harness_type, capabilities[], location_hint?, metadata? }` | `{ harness_id, controller_token, lease_ttl_ms, state_cache_url, transport_hints }` | Harness registration equivalent. |
| `private_beach.list_sessions` | `{ private_beach_id }` | `[{ id, label, media_type, harness_capabilities, controller_id, health }]` | Agents enumerate sessions. |
| `private_beach.subscribe_state` | `{ session_id, mode, format, since_seq? }` | Stream of `{ seq, timestamp, payload, checksum }` | Delivers diff snapshots. |
| `private_beach.queue_action` | `{ session_id, controller_token, actions[] }` | `{ accepted_ids[], rejected[] }` | Agent command submission. |
| `private_beach.ack_actions` | `{ session_id, acks[] }` | `{ status: "ok" }` | Harness ACKs back to manager. |
| `private_beach.acquire_controller` | `{ session_id, requestor_id, ttl_ms?, reason? }` | `{ controller_token, expires_at }` | Lease acquisition handshake. |
| `private_beach.release_controller` | `{ session_id, controller_token }` | `{ released: true }` | Lease release. |
| `private_beach.controller_events.stream` | `{ session_id, cursor? }` | Stream of controller events | UI/agents monitor control changes. |

Schemas for these methods live in `crates/harness-proto` so bindings can be regenerated for Rust/TypeScript clients.

## Open Questions
- Agent attestation: can/should we verify an LLM-runner? Options include allowlist of deployment templates, signed binaries, or human approval per agent.
- Prompt pack storage: store templates in Postgres or bundle within manager image? Need versioning strategy for updates.
- Cross-tenant rate limiting: how to ensure noisy agents on one tenant do not starve others (Redis partitioning, global circuit breakers).
- Offline resilience: what happens if manager loses contact with harness? Should we auto-release leases or freeze state?
- Self-hosting packaging: do we ship Helm charts/docker compose, and how does licensing enforcement work offline?

## Immediate Next Steps
1. Extend the new MCP surface with streaming support (`subscribe_state`, controller event feeds) and publish updated bindings in `crates/harness-proto`.
2. Harden the Redis queue: add queue depth/lag metrics and stalled-consumer alerts now that consumer groups (`XREADGROUP`/`XACK`) and ack metadata are in place.
3. Close the Beach Gate enforcement loop: add RLS/SQLx tests that validate scoped principals against Postgres now that controller events capture `issued_by_account_id`.
4. Extend integration coverage in CI: docker-compose Postgres+Redis smoke tests, queue-depth/lag metrics with alerts, and OpenTelemetry instrumentation stubs for latency tracking.
5. Flesh out agent onboarding templates (Pong demo, SRE runbook) now that prompt scaffolding and auth scopes exist.

## Phase 2 Implementation Plan

### 1. Postgres-Backed Session Registry
- **Status:** Core implementation landed. `AppState` persists sessions, controller leases, runtime metadata, and controller events to Postgres while keeping an in-memory fallback for tests/offline mode.
- **Scope:** Transition `AppState` from the in-memory `HashMap` in `state.rs` to a repository layer backed by SQLx queries over the `session`, `controller_event`, and forthcoming `controller_lease` tables.
- **Data Model Additions:** Introduce a `controller_lease` table (UUID PK, `session_id`, `controller_account_id`, `controller_token`, `issued_by_account_id`, `issued_at`, `expires_at`, `reason`) and optional `session_runtime` table for harness-specific metadata that should survive restarts (`state_cache_url`, `transport_hints`, `last_health_at`). Migrations must enforce RLS mirroring the rest of the schema.
- **Implementation Steps:**
  1. ✅ Define a `SessionRepository`-style facade inside `AppState`; Postgres now handles `register_session`, `list_sessions`, metadata updates, and controller events.
  2. ✅ `RegisterSessionRequest` inserts into `session`, `session_runtime`, and `controller_lease` (new migration `20250219120000_controller_leases_runtime.sql`), with transactional writes + audit events.
  3. ✅ Response DTOs hydrate from DB rows; in-memory structs remain for fallback + tests.
  4. ✅ REST handlers surface database errors through richer `StateError`.
  5. ◻ Add dedicated Postgres integration tests (docker-backed) and `sqlx::query!` assertions; currently only the in-memory flow exercises the router.
- **Follow-Ups:** Wire row-level policy checks against claims, ensure migrations cover backfill paths, and add periodic cleanup for expired leases.

### 2. Redis Cache & Command Queue
- **Status:** Action queues now run on Redis Streams with consumer groups and explicit `ack_actions` handling; health/state payloads sit in TTL'd keys, and the service gracefully degrades when Redis is absent.
- **Scope:** Externalize transient state (`pending_actions`, `last_state`, `last_health`) to Redis so multiple manager instances and harnesses share a consistent view.
- **Key Strategy:** Use namespaced keys `pb:{private_beach_id}:sess:{session_id}`. Store:
  - `actions` as a Redis Stream (XPUSH) to support fan-out and consumer groups for multiple harness workers; acknowledgements use XACK with `pending` semantics.
  - `state` snapshots as a Hash (`state:latest`) plus a capped list (`state:diffs`) for recent diffs (<=200 entries).
  - `health` as a simple hash storing heartbeat payload + timestamp for monitoring.
- **Implementation Steps:**
  1. ✅ `redis::Client` is optional; `AppState::with_redis` gates functionality and logs PING failures.
  2. ✅ `queue_actions`/`poll_actions` now use consumer groups; `ack_actions` issues `XACK`/`XDEL` and trims index mappings.
  3. ✅ Health/state writes go through `SETEX` with TTLs; `session_runtime` mirrors the latest snapshot for warm restart diagnostics.
  4. ◻ Update harness transport hints to advertise stream identifiers; today only REST paths are returned.
  5. ✅ Instrument queue depth and basic counters; add lag metric and alerting next.
- **Open Questions:** Idempotency tokens, multi-harness consumer group design, and eviction strategies for long-lived sessions.

- **Status:** JWT verification is live. `AuthToken` fetches Beach Gate JWKS, caches keys, and falls back to bypass mode if `BEACH_GATE_*` env vars are missing (for local dev).
### 3. Beach Gate JWT Verification
- **Scope:** Replace the permissive `AuthToken` extractor in `routes/auth.rs` with JWT validation against Beach Gate’s JWKS.
- **Implementation Steps:**
  1. ✅ Env-driven config (`BEACH_GATE_JWKS_URL`, `BEACH_GATE_ISSUER`, `BEACH_GATE_AUDIENCE`, `AUTH_BYPASS`) populates an `AuthContext`.
  2. ✅ Tokens are validated via `jsonwebtoken`; JWKS keys cache with TTL refresh.
  3. ✅ REST and MCP handlers now reject requests without the required `pb:*` scopes; controller events capture both controller and issuing account. Remaining work is verifying RLS enforcement via automated tests.
  4. ◻ Persist authenticated principal metadata to Postgres when recording events.
  5. ◻ Add failure-mode/unit tests once a Beach Gate mock is available.
- **Follow-On:** Document local JWKS fixtures and integrate scope checks with RLS enforcement.

### 4. MCP Surface Exposure
- **Scope:** Lift existing REST flows into MCP handlers so automation clients can interact without HTTP glue.
- **Implementation Steps:**
  1. ✅ Inline MCP handler hosted under `/mcp` routes existing REST logic through JSON-RPC 2.0. Shared state/errors now flow through `AppState`.
  2. ✅ Core unary methods (`register_session`, `list_sessions`, `acquire_controller`, `release_controller`, `queue_action`, `ack_actions`) return the same payloads as REST; streaming calls now return SSE URLs.
  3. ◻ Publish schema definitions in `crates/harness-proto`, regenerate bindings, and version the contract.
  4. ✅ Test harness exercises MCP register + list against the in-memory backend; expand to Postgres/Redis once dockerized tests exist.
  5. ◻ Document MCP connection instructions (auth requirements, payload examples) and ensure clients negotiate protocol versions.
- **Compatibility:** REST endpoints stay public beta; MCP currently mirrors only unary flows. Streaming/back-pressure semantics remain a follow-up.

### 5. Agent Onboarding Templates
- **Scope:** Flesh out templated prompt packs to validate the end-to-end control path.
- **Implementation Steps:** Curate `templates/` directory with JSON or YAML definitions, wire them into the onboarding flow, and add regression tests verifying the output contains required bridges and scopes.
- **Dependency:** Requires JWT claims/scopes to map templates to entitlements; coordinate with Beach Gate integration above.

## Current Implementation Status (2025‑10‑24)
- REST API routes remain the primary surface; a JSON-RPC `/mcp` endpoint mirrors the primary control-plane methods for harnesses/agents. Streaming is provided via SSE endpoints, and MCP returns `sse_url` helpers.
- `AppState` now persists sessions, controller leases, and controller events in Postgres via SQLx while preserving an in-memory fallback for tests/offline runs. SQLx `FromRow` models wrap the Postgres queries so downstream clients can reuse the same schema metadata without duplicating SQL.
- New migration `20250219120000_controller_leases_runtime.sql` adds `controller_lease`, `session_runtime`, and `session.harness_type`, keeping the schema aligned with `docs/private-beach/data-model.md`.
- Redis integration powers action queues (Streams) plus TTL’d health/state caches; when Redis is unavailable the manager transparently falls back to in-memory queues.
- Beach Gate JWT verification is wired through `AuthToken`, with JWKS caching and env-based bypass for local development.
- Docker Compose workflow exercises Postgres + Redis; Postgres-backed tests (ignored by default) exist, including RLS enforcement under a limited role.

### Controller Harness Configuration (In Progress)
- Controllers are ordinary Beach sessions whose harness currently holds a lease. To support automation templates we plan to capture, per controller, the following metadata:
  - **Initial prompt** – instructions delivered once the controller lease is granted (e.g., “you orchestrate both Pong paddles…”).
  - **Action prompt template** – concise, structured update that the harness emits whenever idle criteria are met.
  - **Idle detection rules** – predicates (time since diff, regex on terminal output, etc.) that decide when to flush an update.
- Action verbs stay generic (terminal byte writes, keyboard/mouse injections); domain logic lives entirely in the prompt. Spec still needs to document JSON schema and storage (likely `controller_profile` table or JSON column alongside automation assignments).

### Gaps / Follow-up Items
- Fill in Redis consumer group semantics: implement `ack_actions`, dedupe handling, and latency metrics.
- Propagate authenticated principals into controller event rows, enforce per-route scopes, and prove RLS policies with automated tests.
- Expose the documented MCP methods (currently only REST is live) and publish schema updates in `crates/harness-proto`.
- Harden migration automation (CI `sqlx migrate run --check`) and add telemetry wiring (OpenTelemetry spans, Prometheus exporter).
- Build regression tests against the Postgres/Redis path (docker-compose or `sqlx::test`) to guard future refactors.
- Finalize controller-harness prompt/idle schema and wire it into onboarding templates.
- Design the `beach action` CLI passthrough so non-MCP-aware agents can reuse the controller harness channel without bypassing lease enforcement.

## Database Migrations
- Manage schema via `sqlx-cli` migrations stored in `apps/beach-manager/migrations/` (checked in). This repository remains the canonical source of database shape; other services (e.g. `apps/private-beach` via Drizzle ORM) consume the generated SQL rather than defining their own migrations.
- Publish generated schema artifacts (`sqlx-data.json` snapshots, drizzle-friendly SQL dumps) alongside each migration bump so downstream clients can regenerate bindings without drift.
- Initial migration should create all tables/enums described in `docs/private-beach/data-model.md` (organizations, private beaches, memberships, sessions, automation assignments, controller events, file metadata, share links).
- Follow-up migration `20250219120000_controller_leases_runtime.sql` introduces `controller_lease`, `session_runtime`, and `session.harness_type` to support the persisted registry.
- Add supporting indexes for common lookups (`session.private_beach_id`, `controller_event.session_id DESC`, `private_beach_membership.account_id`).
- Enforce row-level security policies in migrations so tests run against production-like posture.
- CI should execute `sqlx migrate run --check` (or equivalent) to detect drift before deploys.

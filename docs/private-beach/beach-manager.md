# Beach Manager Control Plane

## Mission
Beach Manager is the zero-trust control plane that keeps Private Beach cohesive. It is the single source of truth for:
- Session registration, controller leases, and automation assignments.
- Policy enforcement (role-based access, rate limiting, network posture).
- Agent onboarding workflow (prompt packs, MCP bridge wiring, capability gating).
- Audit logging and telemetry routed to billing, alerting, and analytics systems.

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
- **Rate Limits:** dynamic budgets per session and controller; throttle automation loops that risk flooding transports.
- **Audit Completeness:** controller events, state reads, and agent command results must be stored before responses are acknowledged.
- **Secrets Handling:** manager uses short-lived tokens (≤5 min) for agent/harness access; refresh tokens stored encrypted.

## Observability
- OpenTelemetry tracing spans across API layer, Redis command enqueue, harness ACK path.
- Metrics: command latency histograms, queue depth, harness heartbeat freshness, lease churn.
- Structured logs with correlation IDs (private beach, session, controller token) for incident response.

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
1. Implement minimal data model migrations for organizations, private beaches, sessions, controller events.
2. Build harness integration tests using `crates/beach-buggy` to validate registration and action round-trips.
3. Draft agent onboarding templates (Pong demo, support SRE bot) to exercise prompt + capability wiring.

## Database Migrations
- Manage schema via `sqlx-cli` migrations stored in `apps/beach-manager/migrations/` (checked in).
- Initial migration should create all tables/enums described in `docs/private-beach/data-model.md` (organizations, private beaches, memberships, sessions, automation assignments, controller events, file metadata, share links).
- Add supporting indexes for common lookups (`session.private_beach_id`, `controller_event.session_id DESC`, `private_beach_membership.account_id`).
- Enforce row-level security policies in migrations so tests run against production-like posture.
- CI should execute `sqlx migrate run --check` (or equivalent) to detect drift before deploys.

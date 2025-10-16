# Beach Manager Control Plane

## Mission
Beach Manager is the zero-trust control plane that keeps Private Beach cohesive. It is the single source of truth for:
- Session registration, controller leases, and automation assignments.
- Policy enforcement (role-based access, rate limiting, network posture).
- Agent onboarding workflow (prompt packs, MCP bridge wiring, capability gating).
- Audit logging and telemetry routed to billing, alerting, and analytics systems.

## High-Level Architecture
1. **API Layer (Axum)**
   - REST + WebSocket endpoints for the Private Beach web app.
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
  - Private Beach web UI for human workflows.
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

## Open Questions
- Agent attestation: can/should we verify an LLM-runner? Options include allowlist of deployment templates, signed binaries, or human approval per agent.
- Prompt pack storage: store templates in Postgres or bundle within manager image? Need versioning strategy for updates.
- Cross-tenant rate limiting: how to ensure noisy agents on one tenant do not starve others (Redis partitioning, global circuit breakers).
- Offline resilience: what happens if manager loses contact with harness? Should we auto-release leases or freeze state?
- Self-hosting packaging: do we ship Helm charts/docker compose, and how does licensing enforcement work offline?

## Immediate Next Steps
1. Flesh out API contracts (OpenAPI + MCP schema) covering registration, leases, actions, audit queries.
2. Implement minimal data model migrations for organizations, private beaches, sessions, controller events.
3. Build harness integration tests using `crates/beach-buggy` to validate registration and action round-trips.
4. Draft agent onboarding templates (Pong demo, support SRE bot) to exercise prompt + capability wiring.

# Private Beach Roadmap

## Phase 0 – Alignment & Foundations
- ✓ Ratify product boundaries (open-source Beach vs. paid Private Beach scope) and publish guiding principles.
- ✓ Finalize Postgres schema (`docs/private-beach/data-model.md`) and ensure Beach Gate token claims cover required membership roles and share-link flows.
- ✓ Stand up project scaffolding: `apps/private-beach` (Next.js), `apps/beach-manager` service skeleton, shared harness crates/packages.
- ✓ Define engineering cadence, release cadence, and observability baseline (see `docs/private-beach/engineering-cadence.md`).

## Phase 1 – Session Harness (Beach Buggy) MVP
- ✓ Implement `crates/beach-buggy` terminal shim wrapping Beach PTY streams with diff emission + command intake.
- ✓ Extend Beach Cabana with GUI harness module producing metadata descriptors + input injector; establish harness-type taxonomy (`terminal_shim`, `cabana_adapter`, etc.) fed into the manager schema.
- ✓ Wire harness authentication against Beach Gate service accounts; register capabilities through MCP.
- ✓ Ship harness health reporting and attach/detach lifecycle handling (no direct manager UI yet).

## Phase 2 – Beach Manager Core Services
(see `docs/private-beach/beach-manager.md` for architecture details)
- ✅ Postgres-backed session registry + new migrations (`controller_lease`, `session_runtime`) landed; in-memory mode now only used for local tests.
- ✅ Redis Streams + TTL caches power the action/health/state plane with graceful fallback when Redis is offline.
- ✅ Beach Gate JWT verification integrated with JWKS caching; scope checks enforced.
- ✅ JSON-RPC `/mcp` endpoint serves the core `private_beach.*` methods with scope checks.
- ✅ Streaming: SSE endpoints for state and controller events are live; MCP subscription methods return `sse_url` helpers.
- ✅ Metrics: Prometheus counters/gauges at `/metrics` (queue depth, actions enqueued/delivered, health/state counts); Redis reachability gauge.
- ✅ RLS policies applied via migrations; app sets per-request GUC; Postgres RLS tests added (ignored by default) with a limited role.
- Next: publish schema metadata (drizzle-friendly SQL snapshots, enum maps) from `apps/beach-manager/migrations/` so `apps/private-beach` stays in lockstep.
- Next: CI hardening (dockerized Postgres/Redis tests, `sqlx migrate run --check`), and OTEL tracing stubs.
- Next: refine transport hints to include SSE endpoints and stream identifiers.

## Phase 3 – Workspace Shell (Private Beach Surfer)
- ✅ Next.js dashboard scaffolded (`apps/private-beach`) with a sessions view backed by Beach Manager.
- ✅ Live updates via SSE to `/sessions/:id/state/stream` and `/sessions/:id/events/stream`.
- ✅ Controller actions: acquire/release lease wired to REST endpoints.
- ✅ Dev-friendly config: `NEXT_PUBLIC_MANAGER_URL` and localStorage overrides; manager CORS enabled; SSE supports `?access_token=` query for browser auth.

Nice-to-haves (open):
- Real Beach Gate login (OIDC) and token refresh; prefer cookies or Authorization-capable streams over `access_token` query.
- Session search/filtering; richer health/queue indicators and layout grid.
- Controller handoff UX, multi-beach switcher, share-link redemption UI.

## Phase 4 – Private Beach Surfer UX (Dedicated)
- Objectives: deliver a cohesive, production-quality UI/UX for Private Beach that is intuitive, responsive, accessible, and resilient.
- Tracks:
  - Information Architecture: navigation model (beach switcher, sessions, automations, settings), URL structure, deep links.
  - Design System: tokenized color/typography/spacing, components (tiles, badges, toasts, dialogs), dark mode, density scale. Baseline stack: TailwindCSS + shadcn/ui.
  - Session Onboarding: unified “Add Session” flows — By Code (ID+code claim), My Sessions (owned/active sessions), Launch New (CLI guidance with beach binding). Endpoints span Manager + Beach Road; bridge tokens via Beach Gate.
  - Session Surfaces: tile design, health/queue badges, lease countdown, activity glints, selection vs. focus, skeletons/empty/error states.
  - Live Streams: streaming state rendering guidelines, sticky status area, reconnection UX, back-pressure indicators.
  - Controller UX: acquire/release/takeover flows, role visibility, emergency stop affordances and confirmation patterns.
  - Search & Filtering: quick search, filters (harness type, tags, location, status), saved views.
  - Onboarding & Sharing: create beach wizard, invite/share-link flows, role education, success/empty mentorship.
  - Accessibility: WCAG 2.1 AA targets, keyboard navigation, focus rings, reduced motion, ARIA landmarks.
  - Performance: SSR+CSR balance, streaming hydration, code-splitting, per-view performance budgets.
- Deliverables:
  - UX spec with wireframes and component inventory (kept in docs/beach-web-plan.md).
  - Implemented design system + core components in apps/private-beach (TailwindCSS + shadcn/ui configured).
  - Add Session modal (By Code, My Sessions, Launch New) + Manager/Beach Road contracts + bridge token mint/handshake.
  - CLI supports `beach run --private-beach <beach-id>` to auto-register to Manager; UI guides with copyable commands.
  - Polished Sessions Overview and Session Detail views with live status and controls.
  - End-to-end tests for critical flows (acquire/release, stop, search/filter, share-link redemption).
  - Telemetry hooks (UI timings, error rates) and UX health dashboards.

## Phase 5 – Orchestration Mechanics
- ✅ Controller lease countdown in Surfer; emergency stop endpoint (+ UI button) to clear actions and revoke leases.
- ✅ Action queue visualization groundwork: depth and lag (`actions_queue_pending`) metrics added; UI to surface badges next.
- ◻ Latency histograms from `ActionAck` and harness freshness badges in Surfer.
- ◻ Fast-path: design captured to reuse Beach WebRTC signaling with `mgr-actions`/`mgr-state` channels; Redis remains fallback.
- ◻ Expand audit/event views with principals in API responses (controller/issuer IDs) and add list filters/time windows in Surfer.
- ◻ Onboarding UI: templates, capability review, scoped token issuance; expose prompt/idle configuration for controller harnesses.

## Phase 6 – Shared State & Storage
- Deliver minimal key-value API (last-write-wins, per-beach quotas) and file browser for object storage metadata.
- Instrument harness access to shared state via MCP tools (read/write, list, watch).
- Implement retention policies and quota enforcement alarms; expose usage metrics in manager UI.
- Document developer ergonomics for agents leveraging shared state.

## Phase 7 – Agent Workflows & Pong Showcase
- Build automation assignment flows linking agent accounts to sessions/private beaches.
- Implement fast agent manager loop with harness streaming, action dispatch, and controller lease renewals.
- Assemble Pong demo MVP: TUI paddle, Cabana GUI paddle, scoreboard, manager agent, spectator layout.
- Deliver prompt packs + MCP bridge catalog for demo scenarios; allow users to inspect/override prompts.
- Capture telemetry (latency histograms, diff throughput) and package demo runbook for sales/marketing.

## Phase 8 – Operational Hardening & Monetization
- Add billing integration (entitlement sync, usage tracking), plan enforcement, and licensing checks.
- Roll out gated TURN fallback (`private-beach:turn` entitlements, Beach Gate issuance, coturn quotas) for paid tiers.
- Harden deployment: HA Redis/Postgres, disaster recovery playbooks, security audits.
- Ship admin tooling: audit log search, share-link management, support impersonation with guardrails.
- Run load tests (100+ sessions) to validate cache throughput and command latency SLAs.
- Prepare GA release checklist, pricing collateral, and self-hosting story (if applicable).

## Ongoing Cross-Cutting Work
- Documentation & DX (SDK guides, harness integration tutorials, API reference).
- Observability improvements: tracing across harness ↔ manager ↔ UI, alerting thresholds.
- Security reviews: sandbox profiles, secrets handling, RLS test coverage.
- Feedback loops: internal dogfooding, customer council sessions, iterative roadmap adjustments.

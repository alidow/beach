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
- In progress: REST surface implemented with in-memory session registry; Postgres migrations + optional pool wiring landed.
- In progress: Phase 2 implementation plan drafted (`beach-manager.md#phase-2-implementation-plan`) covering Postgres persistence, Redis queues, Beach Gate auth, and MCP exposure.
- Next: persist registry to Postgres, introduce Redis-backed state cache, expose MCP methods, tighten auth (Beach Gate JWTs), and expand integration tests via `manager-sdk`.

## Phase 3 – Workspace Shell (Private Beach Surfer)
- Prototype Next.js dashboard consuming mock data; implement responsive grid, tile management, and status overlays.
- Integrate live data via WebSocket/WebRTC bindings to manager service.
- Support session discovery, search, and basic metadata inspection (capabilities, location hints).
- Add authentication/authorization flows: login with Beach Gate, private beach switching, share-link redemption UI.

## Phase 4 – Orchestration Mechanics
- Implement controller lease countdowns, agent vs. human takeover UX, and emergency stop flows.
- Add action queue visualization (latency, pending command depth) and per-session health indicators.
- Optimize command transport with optional manager↔harness WebRTC data channels; fall back to broker seamlessly.
- Capture controller events into audit log and surface them through UI + API.
- Introduce manager onboarding workflow in the UI (template selection, capability review, scoped token issuance).

## Phase 5 – Shared State & Storage
- Deliver minimal key-value API (last-write-wins, per-beach quotas) and file browser for object storage metadata.
- Instrument harness access to shared state via MCP tools (read/write, list, watch).
- Implement retention policies and quota enforcement alarms; expose usage metrics in manager UI.
- Document developer ergonomics for agents leveraging shared state.

## Phase 6 – Agent Workflows & Pong Showcase
- Build automation assignment flows linking agent accounts to sessions/private beaches.
- Implement fast agent manager loop with harness streaming, action dispatch, and controller lease renewals.
- Assemble Pong demo MVP: TUI paddle, Cabana GUI paddle, scoreboard, manager agent, spectator layout.
- Deliver prompt packs + MCP bridge catalog for demo scenarios; allow users to inspect/override prompts.
- Capture telemetry (latency histograms, diff throughput) and package demo runbook for sales/marketing.

## Phase 7 – Operational Hardening & Monetization
- Add billing integration (entitlement sync, usage tracking), plan enforcement, and licensing checks.
- Harden deployment: HA Redis/Postgres, disaster recovery playbooks, security audits.
- Ship admin tooling: audit log search, share-link management, support impersonation with guardrails.
- Run load tests (100+ sessions) to validate cache throughput and command latency SLAs.
- Prepare GA release checklist, pricing collateral, and self-hosting story (if applicable).

## Ongoing Cross-Cutting Work
- Documentation & DX (SDK guides, harness integration tutorials, API reference).
- Observability improvements: tracing across harness ↔ manager ↔ UI, alerting thresholds.
- Security reviews: sandbox profiles, secrets handling, RLS test coverage.
- Feedback loops: internal dogfooding, customer council sessions, iterative roadmap adjustments.

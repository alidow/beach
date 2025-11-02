# Private Beach Remaining Implementation Plan

## Scope & Intent
- Cover all open roadmap items starting with “NEXT (Critical)” and Phases 4–8 (`docs/private-beach/roadmap.md`).
- Sequence engineering deliverables so we can unlock controller orchestration and the Pong showcase without thrashing teams.
- Capture explicit exit criteria, dependencies, and instrumentation requirements so future contributors can resume quickly.

## Milestone Summary
- **M0 – Server Source of Truth** *(in progress, backend supporting work pending)*  
  Manager-backed CRUD, layout persistence, and auth-hardening that removes the last LocalStorage surfaces.
- **M1 – Surfer UX Foundations (Phase 4)** *(pending)*  
  Ship design system, navigation IA, layout management, and accessibility/performance baselines.
- **M2 – Orchestration Mechanics (Phase 5)** *(pending, some fast-path groundwork shipped)*  
  Fast-path transport closed loop, latency instrumentation, controller UX polish, and onboarding hardening.
- **M3 – Shared State & Storage (Phase 6)**  
  Deliver the scoped KV/file surfaces and associated MCP hooks.
- **M4 – Agent Workflows & Pong Showcase (Phase 7)**  
  Automation assignment loops, demo harnesses, dashboard overlays, and replay/telemetry.
- **M5 – Operational Hardening & Monetization (Phase 8)**  
  Billing, TURN entitlements, HA posture, and admin tooling for GA readiness.

Each milestone rises on the previous one; we only start M4 after the fast-path/UX work in M1–M2 is stable.

---

## M0 — Server Source of Truth (Critical NEXT)
- **Goals**
  - Replace client-side beach/layout storage with Manager-backed CRUD.
  - Enforce membership-aware RLS and scope checks for all Surfer reads/writes.
- **Key Tasks**
  1. Implement `/private-beaches` REST + MCP CRUD with org/account scoping.
  2. Add layout persistence (`GET/PUT /private-beaches/:id/layout`) plus migrations.
  3. Wire Surfer to consume the new APIs; remove LocalStorage fallbacks.
  4. Generate schema artifacts (Drizzle SQL + enum maps) for the UI.
  5. CI hardening: dockerized Postgres/Redis flow, `sqlx migrate run --check`.
- **Dependencies**
  - Existing Postgres schema (`data-model.md`), Clerk/JWKS config.
  - Surfer Postgres connection already landed.
- **Exit Criteria**
  - Surfer renders beaches/layouts solely via Manager APIs.
  - RLS tests cover CRUD paths; bypass mode gated to dev.
  - Schema artifacts published and referenced in Surfer.
- **Risks/Mitigations**
  - *Risk:* RLS regressions. *Mitigation:* Add integration tests + manual QA script.
  - *Risk:* Layout migration churn. *Mitigation:* Maintain migration helpers and fallback seed data.

## Priority Track — Controller Drag & Drop MVP (June 2025)
- **Goal**: Deliver an end-to-end demo-quality workflow where a Private Beach operator can drag one tile onto another in the web app, configure a lightweight control relationship, and have the harness drive the child session over the fast-path transport.
- **Why now**: Enables internal dogfooding and the Pong showcase without waiting for the full GA scope. Builds directly on the fast-path work just landed.
- **Scope (Phase split for parallel teams)**
  - **Track A — Backend & Harness**
    1. Add controller relationship schema (`controller_pairing`) with minimal config (prompt template, update cadence).
    2. REST/MCP endpoints:
       - `POST /sessions/:controller_id/controllers` → validate lease + beach membership, persist pairing, emit events.
       - `DELETE /sessions/:controller_id/controllers/:child_id`.
       - `GET /sessions/:controller_id/controllers`.
    3. Extend harness runtime (CLI + Cabana adapters) to:
       - Subscribe to pairing updates (reuse MCP or add `/controllers/stream` SSE).
       - Auto-renew controller lease and watch child over fast-path.
       - Respect prompt/cadence config (log-friendly prototype).
    4. Emit pairing events/metrics (controller assignment count, fast-path status) and update `fast-path-validation.md` smoke script.
  - **Track B — Web App UX**
    1. ✅ Implement drag-and-drop on the tile canvas (fallback button for accessibility) — shipped via `TileCanvas` DnD handle + Pair button (Codex, 2025-06-24).
    2. ✅ Modal to configure pairing (prompt text, update cadence radio/select) — new `ControllerPairingModal` with accessible selects + API wiring.
    3. ✅ Tile indicators (“controlling X”, “controlled by Y”), quick actions to edit/remove relationships — badges now surface pairings on tiles with configure/remove hooks.
    4. ✅ Drawer section summarising active pairings + status (fast-path vs fallback) using new metrics/events — delivered as controller pairing summary panel with transport badges.
    5. ✅ React Testing Library coverage: drag controller → child, confirm badges + pairing list.
        - NOTE: Frontend now surfaces a `controller pairing API not enabled` warning when Track A endpoints are absent so local devs aren’t blocked; upgrade manager once backend lands.
- **Exit Criteria**
  - Drag/drop controller assignment works locally (Docker + TURN disabled).
  - Harness applies controller actions over fast-path within 1s of child diff; fallback path remains intact.
  - Basic events/metrics land in `/metrics` and `docs/private-beach.log`.
  - Runbook (`docs/private-beach/fast-path-validation.md`) updated with pairing steps.
- **Risks/Mitigations**
  - Schema churn → keep config minimal JSON, mark as experiment.
  - Harness update lag → release CLI/Cabana dev builds alongside feature branch.
  - UX complexity → favour simple modal copy; document future enhancements separately.

## M1 — Surfer UX Foundations (Phase 4)
- **Goals**
  - Establish the dedicated Private Beach design system, IA, and accessibility/performance baseline.
- **Key Tasks**
  0. ✅ Record kickoff brief with IA, design system, and verification assumptions (`docs/private-beach/ux-foundation-brief.md`).
  1. Document IA + navigation spec (sessions, automations, settings).
  2. Pull shadcn/ui + Tailwind tokens into a shared design system package.
  3. Implement layout grid with drag-resize + persistence (backed by M0 APIs).
  4. Build session detail/pop-out view and controller UX polish (handoff, overlays).
  5. Replace query-string tokens with Beach Gate OIDC flows (cookies/headers).
  6. Add search/filter scaffolding (harness type, status, text).
  7. Accessibility audit: keyboard flows, focus rings, reduced motion.
- **Dependencies**
  - M0 layout persistence + schema artifacts.
  - Current Surfer components (`BeachTerminal`, controller drawer).
- **Exit Criteria**
  - FIGMA/wireframes checked in; Surfer matches baseline components.
  - Axe-core lint passes, Lighthouse performance budget documented.
  - Controller takeover UX meets spec; manual QA script recorded.
- **Risks/Mitigations**
  - *Risk:* Component divergence with Cabana. *Mitigation:* Share primitives through existing design system packages.
  - *Risk:* Auth migration friction. *Mitigation:* Dual-run access token + OIDC behind feature flag.

## M2 — Orchestration Mechanics (Phase 5)
- **Goals**
  - Deliver low-latency command path and visibility into action queues and harness freshness.
- **Key Tasks**
  0. ✅ Emit actionable transport hints: manager registration now surfaces fast-path offer/ICE endpoints + channel labels.
 0. ✅ Add harness-side fast-path parsing/scaffold (`crates/beach-buggy/src/fast_path.rs`) for WebRTC negotiation metadata.
 0. ✅ Establish fast-path handshake: harness client negotiates SDP/ICE, provides action stream + ack/state helpers (still pending integration with harness main loop).
  1. **Fast-path transport**
     - ✅ Harness runtime now prefers `mgr-actions`/`mgr-acks`/`mgr-state` data channels with automatic HTTP fallback (`crates/beach-buggy/src/lib.rs`).
     - Manager receive loops parse `mgr-acks` → `ack_actions`, `mgr-state` → `record_state`.
     - Feature flag + telemetry counters (`fastpath_actions_sent_total`, etc.).
  2. **Latency & freshness instrumentation**
     - Action ack histograms, harness freshness badges, queue depth surface in Surfer.
     - Prometheus alerts for pending depth, ack timeout, and freshness thresholds.
     - ✅ Fast-path counters for send/fallback/acks/state + channel closure/error rates exposed in `/metrics`.
  3. **Controller UX**
     - Countdown overlays, manual takeover confirmations, emergency stop validation.
  4. **Session onboarding hardening**
     - Replace dev bridge token with Gate-minted scoped JWT.
     - Enforce ownership checks and persist `session.attach_method`.
  5. **Schema artifact + CI follow-through**
     - Drizzle/enum artifacts automated.
     - Docker-backed integration tests for Postgres/Redis, fast-path mock test.
  6. **Validation & ops guides**
      - ✅ Document automated + manual validation plan, including TURN/STUN knobs (`docs/private-beach/fast-path-validation.md`).
- **Dependencies**
  - Rust manager state (`AppState`), `beach-buggy` harness crate.
  - Surfer telemetry surfaces from M1.
- **Exit Criteria**
  - Fast-path closed loop passes manual test and automated harness mock.
  - Surfer displays queue depth/latency/harness freshness live.
  - Attach flows enforce scoped JWT + audit record.
- **Risks/Mitigations**
  - *Risk:* TURN cost spikes. *Mitigation:* Document TURN-only soak tests + quotas.
  - *Risk:* Harness compatibility gaps. *Mitigation:* Provide CLI feature gate + fallback to Redis path.

## M3 — Shared State & Storage (Phase 6)
- **Goals**
  - Introduce scoped KV + file metadata APIs with quotas and MCP integration.
- **Key Tasks**
  1. Design storage schema + quotas (per beach) and add migrations.
  2. Implement REST + MCP tools for KV read/write/list and file metadata (S3-backed objects).
  3. Extend harness/agent SDKs with shared state helpers.
  4. Telemetry + quota enforcement (metrics + alerts).
  5. Surfer UI for browsing shared state and monitoring usage.
- **Dependencies**
  - Redis/Object storage config, authentication scopes from earlier milestones.
  - Surfer design system for new views.
- **Exit Criteria**
  - End-to-end test: agent writes shared state, Surfer displays, quota meters react.
  - Documentation (DX guide) published.
- **Risks/Mitigations**
  - *Risk:* Storage abuse. *Mitigation:* Strict quotas, rate limits, audit logs.
  - *Risk:* Object store latency. *Mitigation:* Cache metadata in Redis, document expectations.

## M4 — Agent Workflows & Pong Showcase (Phase 7)
- **Goals**
  - Enable orchestrated automation flows and ship the Pong demo as the flagship narrative.
- **Key Tasks**
  1. Automation assignment flows linking accounts to sessions/private beaches; renew controller leases automatically.
  2. Build Pong assets:
     - TUI paddle + scorecard apps (Rust/CLI).
     - Cabana GUI paddle harness instrumentation.
     - Manager agent controller implementing ball physics + action queue loop.
  3. Dashboard overlays: match layout presets, start/stop controls, controller indicators.
  4. Observability: latency tracer, action history log, replay/highlight exporter.
  5. Documentation + runbook for sales/support demos.
- **Dependencies**
  - Fast-path + shared state from M2/M3.
  - Session directory/state streaming MCP APIs (ensure parity with orchestration spec).
- **Exit Criteria**
  - Pong game runs end-to-end with automation default, manual takeover supported.
  - Telemetry dashboards capture latency, queue depth, rally stats.
  - Demo runbook + recorded guide published.
- **Risks/Mitigations**
  - *Risk:* Agent instability. *Mitigation:* Provide fallback deterministic controller and manual override.
  - *Risk:* Demo complexity. *Mitigation:* Feature flag, scripted seeding, automated smoke test.

## M5 — Operational Hardening & Monetization (Phase 8)
- **Goals**
  - Prepare for GA: billing, entitlements, HA posture, and admin tooling.
- **Key Tasks**
  1. Integrate billing pipeline (usage tracking, plan enforcement, entitlements sync).
  2. TURN fallback gating: `private-beach:turn` entitlements, quota management, metrics.
  3. Deployment hardening: HA Postgres/Redis, backup/restore runbooks, DR drills.
  4. Admin tooling: audit log search, share-link management, support impersonation safeguards.
  5. Load testing (100+ sessions) and performance SLA validation.
  6. GA checklist: security review, pricing collateral, self-host story (if applicable).
- **Dependencies**
  - All prior milestones; billing infra + SRE collaboration.
- **Exit Criteria**
  - Billing events flow to finance systems; entitlement enforcement verified.
  - HA posture documented with recovery RTO/RPO targets met in drills.
  - Admin console surfaces audit/search with scoped access.
- **Risks/Mitigations**
  - *Risk:* Entitlement regression. *Mitigation:* Add contract tests and staging soak.
  - *Risk:* Infra complexity. *Mitigation:* Incremental rollout, blue/green deployments.

## Cross-Cutting Threads
- **Documentation/DX:** Update guides after each milestone; ensure API references sync with schema artifacts.
- **Observability:** Expand tracing/metrics dashboards per milestone, culminating in GA SLO coverage.
- **Security:** Regular threat modeling, secret management reviews, and SDL checkpoints before GA.
- **Change Management:** Adopt feature flags + staged rollouts for user-facing shifts (auth, fast-path, demo toggles).

## Next Actions (June 2025)
1. Finalize M0 deliverables (CRUD, layout persistence, RLS tests, schema artifacts).
2. Stand up fast-path harness prototype while UX team lands the design system scaffolding.
3. Draft Pong implementation issues referencing this plan; sequence work behind M2 dependencies.
4. Schedule TURN-only soak test and load-test rehearsal to validate fast-path resilience before demo work.

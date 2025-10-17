# Beach WebSocket Fallback (Beach Lifeguard) – Phased Plan

Status: Proposed  
Date: 2025-10-11  
Author: Codex (implementation planning)

## Overview
- Provide an absolute-last-resort WebSocket transport when dual-channel WebRTC cannot connect or sustain throughput.
- Stand up a new high-efficiency WebSocket relay service outside the `beach` and `beach-road` repos (service name **beach-lifeguard**, formerly codenamed beach-buoy).
- Extend Beach clients/servers with a minimal, power-conscious fallback path that preserves current security guarantees and observability.
- Maintain WebRTC as the default; fallback must remain dormant unless explicit guardrails are met.

## Status Snapshot
- Current Phase: 2 – Data Plane & Reliability Enhancements (session registry + flow control in place, idle recycler online)
- Completed: Phase 0 decisions, Phase 1 skeleton deliverables, Phase 2 routing/backpressure upgrades (DashMap/Slab registry, bounded channels, flow-control metrics), idle recycling with automated close frames, Redis + Prometheus telemetry (`/metrics`), optional OpenTelemetry stdout exporter, `/healthz` & `/debug/stats`, activation gating via Beach Gate entitlements (JWT proof → `fallback_authorized` bit enforced end-to-end)
- Next: validate in dev/staging with the load harness, finish dashboards/runbook polish, then advance to Phase 3 client/server fallback integration

## Progress Summary
- **Token exchange & guardrails:** `/v1/fallback/token` lives in beach-road, issues Redis-backed soft guardrail tokens (currently JSON + base64). CLI negotiator consumes the token before attempting fallback.
- **Relay prototype:** beach-lifeguard registers each WebSocket peer, echoes `ServerHello`, and broadcasts frames to the rest of the session while bumping Redis counters (`fallback:metrics:*`). Dev overrides live in `BEACH_LIFEGUARD_DISABLE_OIDC`.
- **Flow control & routing:** session registry now uses a DashMap + Slab fan-out table with bounded per-connection channels, flow-control drop metrics, and a recycler loop that emits idle closes (`close_code::POLICY`).
- **Operator visibility:** `/healthz` for readiness, `/debug/stats` for active session inventory, and `/metrics` (Prometheus scrape) covering handshakes, connection lifecycle, and fan-out counters; OpenTelemetry stdout exporter available via `BEACH_LIFEGUARD_OTEL_STDOUT=1` for trace debugging.
- **Deployment posture:** production deployment stays paused until the relay is exercised end-to-end in the dev environment; manifests remain staging-only.
- **What remains:** stand up the load/perf harness + dashboards, finish SRE runbook polish, and move into Phase 3 integration work once dev/staging validation passes.
- **Control-plane kill switch:** beach-road now honours `FALLBACK_WS_PAUSED`; when enabled the `/fallback/token` endpoint refuses requests with a structured 403 so operators can halt fallback minting instantly. Entitlement proofs are required whenever `FALLBACK_REQUIRE_OIDC=1`, and verified tokens set a `fallback_authorized` bit consumed by the lifeguard handshake.
- **CLI & web toggles:** terminal fallback requests now honour CLI flags (`--fallback-cohort`, `--fallback-entitlement-proof`, `--fallback-telemetry-opt-in`) alongside Beach Auth profiles, and beach-surfer exposes matching advanced inputs plus a Beach Auth call-to-action when fallback is locked — giving rollout teams a UI/CLI surface while full browser auth lands; paused responses surface a friendly error to users.
- **Token metrics:** beach-road exports `beach_fallback_token_requests_total` via `/metrics`, labelled by `outcome` (`issued`, `paused`, `entitlement_denied`, `invalid_session`, `error`) and guardrail state, so dashboards can track kill-switch use and entitlement failures.
- **Browser plumbing:** advanced overrides from beach-surfer are now injected into the connection controller and logged alongside transport setup, ready to be forwarded once the browser fallback transport is wired to beach-lifeguard.
## Naming Options
- **beach-lifeguard** (selected): Signals emergency-only fallback, aligns with “Plan B” intent.
- **beach-buoy** (previous codename): Matches rescue/fallback metaphor, short and descriptive.
- **ripcord**: Emergency-only connotation without implying constant intervention.
- **breakwater**: Defensive structure that absorbs impact when seas are rough.
- **backline**: The last defensive line in surfing; signals "only when we have to."

## Phase 0 – Architecture Alignment & Guardrails
**Goal:** Lock in detection logic, transport contracts, and service boundaries before implementation.
- Document the fallback decision flow (timers, retry counts, telemetry signals) and how we surface it to operators.
- Define the minimal message framing and auth shared between Beach and beach-lifeguard (token exchange, encryption envelope, compression expectations).
- Specify repo layout for beach-lifeguard (Rust workspace, `benches/`, `load/`), CI story, and release artifacts (container + binary).
- Draft rollout guardrails: feature flag plumbing, kill switch, alert thresholds, logging redaction rules.
**Deliverables:** `docs/beach-lifeguard/architecture.md`, updated transport diagrams, ADR covering fallback policy.  
**Exit Check:** Review sign-off from transport + infra maintainers; guardrails approved.

### Phase 0 Task Tracker
| Task | Owner | Status | Notes |
| --- | --- | --- | --- |
| Finalize service name | Transport WG | Completed | Chosen name: `beach-lifeguard` (2025-10-11) |
| Draft fallback decision flow diagram | Codex | Completed | Sequence diagram captured in `docs/beach-lifeguard/architecture.md` |
| Define shared auth + framing contract | Codex | Completed | Handshake/frame spec accepted |
| Propose beach-lifeguard repo layout & CI plan | Codex | Completed | Workspace/CI expectations captured |
| Guardrail checklist (cohort flag, kill switch, alerts) | Codex | Completed | Policy approved (default flag false, dev override noted) |
| Circulate architecture draft for feedback | Codex | Completed | Internal review request sent; awaiting external comments |
| Draft ADR `docs/beach-lifeguard/adr-2025-10-beach-lifeguard-fallback.md` | Codex | Completed | ADR accepted 2025-10-11 |
| Log cross-team tracking issues | Codex | Completed | TP-4821 (control plane), INF-3377 (infra), SEC-2190 (security) |

### Phase 1 Task Tracker
| Task | Owner | Status | Notes |
| --- | --- | --- | --- |
| Scaffold `beach-lifeguard` crates & dev tooling | Transport WG + Infra | Completed | Added under `apps/beach-lifeguard/` with Docker Compose stub service |
| Implement beach-lifeguard server handshake & relay skeleton | Transport WG | Completed | Axum-based WSS endpoint validating tokens, broadcasting frames, exposing `/healthz` & `/debug/stats` |
| Define control-plane fallback token API (TP-4821) | Platform Control Plane | Completed | `/fallback/token` endpoint in beach-road with Redis guardrails + OIDC toggle |
| Integrate fallback transport path in beach client/server | Beach Maintainers | Completed | `/fallback/token` handshake + beach-lifeguard relay wired into terminal negotiator (feature flag follow-up TBD) |
| Instrument beach-lifeguard telemetry/guardrail emissions | Transport WG | Completed | Redis counters plus Prometheus `/metrics` (handshakes, fan-out, bytes) and optional stdout OpenTelemetry exporter |
| Draft initial load-testing plan & schedule review | Transport QA | Deferred | Revisit during Phase 2 reliability kickoff; targets captured in architecture |
| Prepare development feature flag strategy docs | Beach Maintainers | Completed | Architecture doc outlines `BEACH_LIFEGUARD_DISABLE_OIDC` + compose defaults |
| Collect reviewer feedback (transport/infra/security) | Codex | Completed | Transport/Infra/Security sign-off: handshake ladder locked, ECS rollout deferred until post-dev validation, telemetry plan acknowledged (2025-10-18) |

### Phase 2 Task Tracker
| Task | Owner | Status | Notes |
| --- | --- | --- | --- |
| Build session registry with bounded fan-out channels | Transport WG | Completed | DashMap + Slab registry replaces RwLock Vec, per-connection mpsc queues enforce backpressure |
| Emit flow control metrics and idle recycler closes | Transport WG | Completed | Drop counters + idle pruning close frames surfaced via Prometheus (`beach_lifeguard_*`) |
| Wire chaos/load harness scaffolding | Transport QA | Planned | Harness to exercise 5K active sessions scheduled post dev validation |

## Phase 1 – beach-lifeguard Service Skeleton
**Goal:** Create a production-ready foundation for the external WebSocket relay.
- Stand up Rust crate(s) using `tokio` + `hyper`/`axum` + `tokio-tungstenite` (or `hyper` HTTP upgrade) with zero-copy frame handling and backpressure.
- Implement minimal `CONNECT` handshake: mutual auth via pre-issued session token, capability negotiation (protocol version, compression), heartbeat contract.
- Wire structured telemetry (OpenTelemetry traces, Prometheus metrics, structured logs) and feature flag toggles.
- Build CI jobs (lint, tests, load test smoke) and container build pipeline.
**Deliverables:** Public repo `beach-lifeguard`, documented handshake contract, `cargo bench` baseline for 10k concurrent idle connections.  
**Exit Check:** Load test sustaining 10k connections on dev hardware with <5% CPU and consistent latency budget.

## Phase 2 – Data Plane & Reliability Enhancements
**Goal:** Make beach-lifeguard production-resilient for thousands of concurrent sessions.
- Implemented session routing tables with DashMap + Slab fan-out and per-connection bounded queues (flow-control metrics + drops surfaced in Prometheus).
- Added fast-path relay instrumentation that tracks delivered bytes, backpressure pressure, and idle recycler kicks; idle peers receive automated close frames (`policy`).
- Compression negotiation remains pass-through for now; delta/binary packing deferred to targeted load scenarios once Phase 3 clients land.
- Idle detection + recycler loop prunes dormant sockets while logging session drain events to prevent reconnect storms.
**Deliverables:** Benchmarks demonstrating 5K concurrent active sessions with P95 end-to-end latency <40ms under 64KB/s output streams.  
**Exit Check:** Chaos tests (random disconnects, CPU spikes) maintain >99.5% successful reconnects, no memory leaks in soak tests. *(Remaining work: run load/chaos harness in dev/staging to sign off targets.)*

## Phase 3 – Beach Client & Server Integration
**Goal:** Introduce WebSocket fallback paths while keeping WebRTC default.
- Extend transport abstraction (e.g., `TransportKind`) in `apps/beach` and `apps/beach` to include `WebSocketFallback`.
- Implement fallback controller: attempt WebRTC N times with backoff; only initiate WebSocket handshake after failure quorum and feature flag.
- Hook up serialization/deserialization for WebSocket frames in the existing transport pipeline; ensure encryption and auth reuse the same primitives as WebRTC.
- Surface explicit UI/CLI signals when fallback is active and emit structured telemetry events for observability.
**Deliverables:** Feature-flagged fallback branch with full test coverage (unit + integration harness using mocked beach-lifeguard).  
**Exit Check:** `cargo test --workspace` + dedicated transport integration suite verifying fallback engages only after forced WebRTC failure.

## Phase 4 – Observability, Tooling & Load Validation
**Goal:** Ensure operators can trust and manage fallback engagements safely.
- Add dashboards (Grafana) for fallback activation rate, session duration, concurrency, and error codes.
- Implement active health checks and synthetic probes hitting both WebRTC and WebSocket paths.
- Run large-scale load tests (e.g., k6/Vegeta) validating fallback performance at target concurrency with realistic payloads.
- Build automated regression suite comparing latency/CPU impact between WebRTC and fallback under the same workloads.
**Deliverables:** Dashboard bundle, load-test reports committed under `docs/perf`.  
**Exit Check:** Fallback activation stays below agreed threshold (<0.5% of sessions) during synthetic chaos weeks with alerts wired.

## Phase 5 – Rollout & Operational Hardening
**Goal:** Ship fallback safely and keep it as a last-resort escape hatch.
- Stage rollout: dogfood, limited cohort, GA—monitor fallback triggers, manual approvals for expanding cohorts.
- Document runbooks (trigger triage, manual disable, scaling beach-lifeguard pool) and on-call training.
- Establish auto-disable actions if fallback usage spikes or telemetry missing.
- Evaluate long-term maintenance (region redundancy, cost modeling, dependency audits).
**Deliverables:** Runbook in `docs/operations/beach-lifeguard-runbook.md`, post-rollout review template.  
**Exit Check:** Production incident drill proves we can detect, respond to, and roll back fallback within SLA; compliance/security sign-off received.

## Next Steps After Approval
1. Harden operability: wire the new Prometheus metrics into dashboards/alerts, add a `/debug` surface for paused state, and finish the SRE runbook.
2. 📦 Gate fallback behind Clerk-backed entitlements: **Done (dev/staging)** – beach-road now verifies Beach Gate JWTs, issues tokens with a `fallback_authorized` feature bit, and the lifeguard handshake refuses unauthorized clients. Follow-up: wire cohort toggles/metrics dashboards before GA.
3. Thread browser overrides through the upcoming WSS transport handshake so web clients pass cohort/proof/telemetry alongside the CLI when beach-lifeguard activates.
4. Exercise the load/chaos harness against the new session registry (target 5K active / 10K idle) and archive reports ahead of Phase 3 rollout planning.

## Open Source Coordination
- apps/beach is slated for open sourcing while `apps/beach-lifeguard` stays private; the public repo will only ship the lightweight `beach-lifeguard-client` crate needed for handshake message shapes and local testing helpers.
- The client crate must remain protocol-only (no guardrail math, Redis keys, routing internals) so the proprietary relay logic stays encapsulated behind the `/fallback/token` API and WSS endpoint.
- Fallback negotiation in the open project remains strictly optional and feature-flagged, allowing downstream users to disable it when the private beach-lifeguard service is unavailable.
- Publish a short “Using beach-lifeguard fallback” guide alongside the public release to document the boundary and keep expectations clear without exposing private implementation details.

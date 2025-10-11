# Beach WebSocket Fallback (Beach Rescue) – Phased Plan

Status: Proposed  
Date: 2025-10-11  
Author: Codex (implementation planning)

## Overview
- Provide an absolute-last-resort WebSocket transport when dual-channel WebRTC cannot connect or sustain throughput.
- Stand up a new high-efficiency WebSocket relay service outside the `beach` and `beach-road` repos (service name **beach-rescue**, formerly codenamed beach-buoy).
- Extend Beach clients/servers with a minimal, power-conscious fallback path that preserves current security guarantees and observability.
- Maintain WebRTC as the default; fallback must remain dormant unless explicit guardrails are met.

## Status Snapshot
- Current Phase: 0 – Architecture Alignment & Guardrails (decisions approved)
- Completed: Naming selection, architecture/guardrail design, ADR acceptance (2025-10-11)
- Next: Kick off cross-team implementation work for Phase 1 (repo scaffolding, control-plane updates)

## Naming Options
- **beach-rescue** (selected): Signals emergency-only fallback, aligns with “Plan B” intent.
- **beach-buoy** (previous codename): Matches rescue/fallback metaphor, short and descriptive.
- **ripcord**: Emergency-only connotation without implying constant intervention.
- **breakwater**: Defensive structure that absorbs impact when seas are rough.
- **backline**: The last defensive line in surfing; signals "only when we have to."

## Phase 0 – Architecture Alignment & Guardrails
**Goal:** Lock in detection logic, transport contracts, and service boundaries before implementation.
- Document the fallback decision flow (timers, retry counts, telemetry signals) and how we surface it to operators.
- Define the minimal message framing and auth shared between Beach and beach-rescue (token exchange, encryption envelope, compression expectations).
- Specify repo layout for beach-rescue (Rust workspace, `benches/`, `load/`), CI story, and release artifacts (container + binary).
- Draft rollout guardrails: feature flag plumbing, kill switch, alert thresholds, logging redaction rules.
**Deliverables:** `docs/beach-rescue-architecture.md`, updated transport diagrams, ADR covering fallback policy.  
**Exit Check:** Review sign-off from transport + infra maintainers; guardrails approved.

### Phase 0 Task Tracker
| Task | Owner | Status | Notes |
| --- | --- | --- | --- |
| Finalize service name | Transport WG | Completed | Chosen name: `beach-rescue` (2025-10-11) |
| Draft fallback decision flow diagram | Codex | Completed | Sequence diagram captured in `docs/beach-rescue-architecture.md` |
| Define shared auth + framing contract | Codex | Completed | Handshake/frame spec accepted |
| Propose beach-rescue repo layout & CI plan | Codex | Completed | Workspace/CI expectations captured |
| Guardrail checklist (cohort flag, kill switch, alerts) | Codex | Completed | Policy approved (default flag false, dev override noted) |
| Circulate architecture draft for feedback | Codex | Completed | Internal review request sent; awaiting external comments |
| Draft ADR `docs/adr/2025-10-beach-rescue-fallback.md` | Codex | Completed | ADR accepted 2025-10-11 |
| Log cross-team tracking issues | Codex | Completed | TP-4821 (control plane), INF-3377 (infra), SEC-2190 (security) |

### Phase 1 Task Tracker (Prep)
| Task | Owner | Status | Notes |
| --- | --- | --- | --- |
| Scaffold `beach-rescue` crates & dev tooling | Transport WG + Infra | Completed | Added under `apps/beach-rescue/` with Docker Compose stub service |
| Define control-plane fallback token API (TP-4821) | Platform Control Plane | Completed | `/fallback/token` endpoint in beach-road with Redis guardrails + OIDC toggle |
| Draft initial load-testing plan & schedule review | Transport QA | Deferred | Will revisit post feature completion; targets documented in architecture |
| Prepare development feature flag strategy docs | Beach Maintainers | Completed | Architecture doc outlines `BEACH_RESCUE_DISABLE_OIDC` + compose defaults |
| Collect reviewer feedback (transport/infra/security) | Codex | In Progress | Reviews due 2025-10-18; incorporate into Phase 1 kickoff briefing |

## Phase 1 – beach-rescue Service Skeleton
**Goal:** Create a production-ready foundation for the external WebSocket relay.
- Stand up Rust crate(s) using `tokio` + `hyper`/`axum` + `tokio-tungstenite` (or `hyper` HTTP upgrade) with zero-copy frame handling and backpressure.
- Implement minimal `CONNECT` handshake: mutual auth via pre-issued session token, capability negotiation (protocol version, compression), heartbeat contract.
- Wire structured telemetry (OpenTelemetry traces, Prometheus metrics, structured logs) and feature flag toggles.
- Build CI jobs (lint, tests, load test smoke) and container build pipeline.
**Deliverables:** Public repo `beach-rescue`, documented handshake contract, `cargo bench` baseline for 10k concurrent idle connections.  
**Exit Check:** Load test sustaining 10k connections on dev hardware with <5% CPU and consistent latency budget.

## Phase 2 – Data Plane & Reliability Enhancements
**Goal:** Make beach-rescue production-resilient for thousands of concurrent sessions.
- Implement session routing tables with lock-free data structures (slab + atomic indices) to keep per-message overhead minimal.
- Add delta compression / binary frame packing to align with Beach's transport enums.
- Provide fast-path delivery for multiplexed channels (control vs output) and enforce per-session flow control.
- Integrate connection recycling, idle detection, and soft backoff to avoid cascading reconnect storms.
**Deliverables:** Benchmarks demonstrating 5K concurrent active sessions with P95 end-to-end latency <40ms under 64KB/s output streams.  
**Exit Check:** Chaos tests (random disconnects, CPU spikes) maintain >99.5% successful reconnects, no memory leaks in soak tests.

## Phase 3 – Beach Client & Server Integration
**Goal:** Introduce WebSocket fallback paths while keeping WebRTC default.
- Extend transport abstraction (e.g., `TransportKind`) in `apps/beach` and `apps/beach-human` to include `WebSocketFallback`.
- Implement fallback controller: attempt WebRTC N times with backoff; only initiate WebSocket handshake after failure quorum and feature flag.
- Hook up serialization/deserialization for WebSocket frames in the existing transport pipeline; ensure encryption and auth reuse the same primitives as WebRTC.
- Surface explicit UI/CLI signals when fallback is active and emit structured telemetry events for observability.
**Deliverables:** Feature-flagged fallback branch with full test coverage (unit + integration harness using mocked beach-rescue).  
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
- Document runbooks (trigger triage, manual disable, scaling beach-rescue pool) and on-call training.
- Establish auto-disable actions if fallback usage spikes or telemetry missing.
- Evaluate long-term maintenance (region redundancy, cost modeling, dependency audits).
**Deliverables:** Runbook in `docs/operations/beach-rescue-runbook.md`, post-rollout review template.  
**Exit Check:** Production incident drill proves we can detect, respond to, and roll back fallback within SLA; compliance/security sign-off received.

## Next Steps After Approval
1. Scaffold `beach-rescue` crates and local dev tooling inside the monorepo; integrate with existing Docker Compose (`redis`) for easy spin-up.
2. Implement control-plane fallback token API backed by Redis guardrail counters, with optional OIDC/Clerk entitlement proof and `BEACH_RESCUE_DISABLE_OIDC` override for dev.
3. Defer large-scale load testing until post-feature completion; maintain target metrics documentation for future GA readiness.

# ADR 2025-10: WebSocket Fallback Service ("beach-rescue")

Status: Accepted  
Date: 2025-10-11  
Deciders: Transport WG, Infra Core, Security Guild  
Consulted: Beach maintainers, Platform control-plane owners  
Approved By: beach product owner (2025-10-11)

## Context
- WebRTC remains the primary transport for Beach, but certain environments (locked-down corporate networks, NAT corner cases, temporary TURN outages) block data channel establishment.
- Current behavior is repeated WebRTC retries with no alternate path, leading to session failures.
- We must preserve the zero-trust posture defined in `docs/secure-shared-secret-webrtc-plan.md` while offering a fallback that scales to thousands of concurrent connections.
- Requirements include: isolation from existing repos, strict guardrails, minimal operational surface, and compatibility with existing transport abstractions.

## Decision
1. Introduce an external WebSocket relay service, working name **beach-rescue**, hosted in its own repository and deployment pipeline.
2. Gate fallback activation behind:
   - Control-plane cohort flag `fallback_ws_enabled` issued to paying/entitled users (future “private beaches” OIDC rollout); default `false` system-wide with a dev-only override env var.
   - Rolling activation monitoring (≥0.5% sessions per hour triggers soft guardrail alerts while still issuing tokens), with aggregated, privacy-preserving telemetry and opt-out respected.
3. Use shared pre-shared-key material from the secure WebRTC plan for token issuance and frame encryption, keeping beach-road unaware of secrets.
4. Implement a strict failover ladder: repeated WebRTC attempts → TURN fallback → beach-rescue only after guardrail checks succeed.
5. Provide explicit observability (telemetry events, dashboards, alerts) and a kill switch that halts token minting instantly.
6. Deploy beach-rescue on AWS ECS Fargate (per-environment clusters) with GitHub Actions CI, Cosign-signed images in `ghcr.io/beach/beach-rescue`, and plan migration to Kubernetes once stable.
7. Define control-plane fallback token API (`POST /v1/fallback/token`) returning Ed25519-signed CBOR tokens valid for ≤5 minutes, backed by Redis guardrail counters and respecting telemetry opt-out.
8. Establish load-testing objectives: 10k idle / 5k active sessions, k6 + Rust harness tooling, chaos scenarios, and 24h soak without leaks.

## Consequences
- **Positive**
  - Users receive a reliable escape hatch when WebRTC fails, reducing session drop-offs.
  - Architecture keeps signaling server zero-trust while adding minimal new dependencies to the Beach repo.
  - Operational guardrails limit risk of runaway fallback usage and make detection actionable.
- **Negative**
  - Additional service to maintain (CI/CD, on-call rotation, infrastructure costs).
  - Increased complexity in control plane (token minting, budget tracking).
  - Need for high-efficiency WebSocket stack capable of sustaining target load; performance regressions could impact emergency scenarios.
- **Open Items**
  - Execute implementation tasks tracked in TP-4821 (control plane), INF-3377 (infra), SEC-2190 (security) to operationalize the approved design.

## Alternatives Considered
- **Keep WebRTC-only with improved TURN coverage:** Does not solve air-gapped/firewalled cases where TURN also fails; still leaves users stranded.
- **Integrate WebSocket fallback inside beach-road:** Violates repo separation requirement and mixes responsibilities, complicating scaling.
- **Use third-party WebSocket providers:** Adds external dependency, harder to guarantee zero-trust and performance budgets.

## Implementation Notes
- Architecture specifics captured in `docs/beach-rescue/architecture.md`.
- Phase planning tracked in `docs/beach-rescue/plan.md`.
- Load test and observability requirements to be finalized during Phase 1 review.

## Review Plan
- Circulate ADR alongside architecture draft for comments by 2025-10-18.
- Incorporate feedback, then move ADR to **Accepted** prior to Phase 1 kickoff.

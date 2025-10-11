# beach-rescue Architecture Outline

Status: Ready for stakeholder review (Phase 0)  
Date: 2025-10-11  
Owner: Codex (transport planning)

## Purpose
- Capture the technical boundaries between Beach, the new `beach-rescue` WebSocket relay, and existing infrastructure.
- Describe the failover detection logic and guardrails that keep WebSocket fallback as an absolute last resort.
- Provide implementation scaffolding (contracts, repo layout, observability) ahead of Phase 1 execution.

## Fallback Decision Flow (Draft)
- **Trigger inputs:** consecutive WebRTC offer/answer failures, ICE timeout spikes, transport-level telemetry (packet loss > threshold), manual override flag.
- **Escalation ladder:** retry WebRTC `N` times with exponential backoff → attempt TURN relay if available → only then initiate beach-rescue handshake.
- **Activation checks:** feature flag enabled, guardrail budget not exceeded, no active incident suppressing fallback.
- **Telemetry:** emit structured event `transport.fallback.considered` with reason codes; promote to `transport.fallback.activated` only after successful WebSocket session creation.

### Fallback Sequence (Draft)

```mermaid
sequenceDiagram
    participant Host as Beach Host
    participant Client as Beach Participant
    participant ControlPlane as Beach Control Plane
    participant Rescue as beach-rescue

    Host->>ControlPlane: Request session create (includes secure signaling)
    ControlPlane-->>Host: Session ID + fallback token (sealed)
    ControlPlane-->>Client: Session info + fallback token via secure signaling

    Host->>Client: Attempt WebRTC offer/answer exchange
    Host-->>Host: Retry WebRTC N times (exponential backoff)
    Note over Host,Client: TURN relay attempt (if configured)
    Host-->>Host: Guardrail check (fallback allowed?)

    alt Guardrails permit fallback
        Host->>Rescue: WSS ClientHello (token, nonce, version)
        Rescue-->>Host: ServerHello (nonce, features)
        Rescue->>ControlPlane: Token validate (signature, expiry, budget)
        Host-->>Client: Signal fallback activation
        Client->>Rescue: WSS ClientHello (token, nonce, version)
        Rescue-->>Client: ServerHello
        Host<->>Rescue: Encrypted transport frames (control/data)
        Client<->>Rescue: Encrypted transport frames (control/data)
    else Guardrail tripped
        Host-->>Client: Stay on degraded WebRTC / retry schedule
    end

    Host-->>Client: Close session (propagate CLOSE frame)
    Rescue-->>Host: CLOSE reason (NORMAL or guarded)
    Rescue-->>Client: CLOSE reason
```

## Shared Auth & Message Framing (Draft)
- **Identity & token distribution**
  - Beach host requests a short-lived fallback token from the coordination service (`beach` API) during session creation; token is derived from the same Argon2id pre-shared material used for secure WebRTC (see `docs/secure-shared-secret-webrtc-plan.md`).
  - Token payload: `session_id`, `expires_at`, `capabilities = ["fallback-ws"]`, `signature` (Ed25519 using Beach control plane key). Token is delivered to participants via the existing secure signaling envelope so `beach-road` never sees it.
- **Client ↔ beach-rescue handshake**
  1. Client opens HTTPS connection, upgrades to WSS, and sends `ClientHello { session_id, token, client_nonce, protocol_version, compression=none|brotli }`.
  2. beach-rescue validates token signature and expiry, checks guardrail budget, returns `ServerHello { ack_nonce, server_nonce, negotiated_compression, features_bitmap }`.
  3. Both sides derive `hkdf(psk, "beach-rescue/handshake", client_nonce || server_nonce)` to obtain `K_send`/`K_recv` and 96-bit starting nonces; all subsequent frames use AES-GCM (default) with option to negotiate ChaCha20-Poly1305 when clients prefer it.
- **Frame format**
  - `Frame = header || ciphertext || tag` where header packs:
    - `u8 version`
    - `u8 frame_kind` (`0=control`, `1=data`, `2=heartbeat`, `3=close`)
    - `u16 payload_len`
    - `u32 session_seq` (per-direction monotonic counter, wraps handled via rekey)
  - Payload defaults to the same protobuf/cbor structure already used for Beach transport messages; compression flag triggers brotli on payload before encryption.
- **Heartbeats & close**
  - Heartbeats send empty payload frame every 15s; missing three heartbeats triggers reconnect attempt (still honoring guardrails).
  - Close frames carry reason codes (`NORMAL`, `RATE_LIMITED`, `GUARDRAIL_TRIPPED`, `SERVER_SHUTDOWN`).

## beach-rescue Repo Layout (Draft)
- **Workspace structure (in-monorepo under `apps/beach-rescue/`)**
  - `apps/beach-rescue/core` – transport framing, auth, metrics, shared config.
  - `apps/beach-rescue/server` – Axum/hyper server binary with WSS upgrade pipeline.
  - `apps/beach-rescue/client` – lightweight client harness used by integration tests (published as workspace crate).
  - `load/` – optional k6 + Rust load harness (added near GA).
  - `scripts/` – local dev helpers (build, run, generate self-signed certs).
- **Build & Runtime**
  - Reuse existing workspace tooling (`cargo fmt`, `cargo clippy`, `cargo test`) from the monorepo; no dedicated CI pipeline yet.
  - Local development uses Docker Compose alongside existing Redis service; server binary can run natively or via compose service `beach-rescue`.
  - Deployment manifests deferred until production rollout; target platform AWS ECS Fargate with future path to Kubernetes (documented but not implemented yet).

## Guardrails & Operational Policy (Draft)
- **Feature gating**
  - Control-plane cohort flag `fallback_ws_enabled` issued only to entitled accounts (initially paying users / “private beaches” cohort); default `false` for all sessions.
  - Future-friendly design: flag derivation must occur post-authentication (OIDC) so entitlements follow the user regardless of session host.
  - Development override: in non-production builds an explicit env var (`BEACH_DEV_ENABLE_FALLBACK_WS=1`) may force-enable the flag for testing; production ignores this override.
- **Activation monitoring**
  - Rolling window guardrail: alert when fallback activations exceed 0.5% of sessions per hour; tokens continue to be minted but events are tagged `guardrail_soft_breach` and escalate to on-call.
  - Provide manual override (`fallback_ws_paused`) for SREs to stop token issuance during runaway incidents; default state keeps fallback available for users behind restrictive firewalls.
  - Distinguish paid vs unpaid cohorts in metrics (aggregate only) so we can see if entitlements correlate with fallback volume; unpaid users have the flag unset and are excluded from WSS paths.
- **Kill switch**
  - Emergency feature flag in control-plane API to disable token issuance; clients degrade to WebRTC retries only.
  - SRE runbook for manual draining (mark all sessions `draining`, send CLOSE frames, scale deployment to zero).
- **Alerting**
  - Prometheus alerts on high activation rate (soft breach), connection churn > 20%/5min, CPU > 70%, and handshake failure spikes; alerts page the transport on-call rotation.
  - Structured logs shipped to Loki; `transport.fallback.activated` / `guardrail_soft_breach` events feed PagerDuty + Slack. Metrics emitted as privacy-preserving aggregates (counts per cohort, no user identifiers) with opt-out respected (clients can disable telemetry, which suppresses non-essential metrics).
- **Rollout gates**
  - Must pass soak test (48h) in staging with synthetic clients before first prod cohort.
  - Require sign-off from security (key handling review) and transport leads (perf budgets).

## Control Plane & Token API Decisions
- **Endpoint:** `POST /v1/fallback/token` (authenticated via mTLS service account) returning `{ token, expires_at, cohort, guardrail_hint }`.
- **Inputs:** `session_id`, `cohort_id`, `client_version`, `entitlement_proof` (optional JWT from future Clerk/OIDC integration), `telemetry_opt_in`.
- **Validation Flow:** verify entitlement when proof provided (paid “private beaches” users); in dev or when entitlement disabled, skip OIDC checks. Ensure `fallback_ws_enabled` true. Guardrail counters stored in Redis (shared with beach-road) using hourly buckets.
- **Token Format:** Ed25519-signed CBOR payload containing `session_id`, `issued_at`, `expires_at` (<= 5 min), `cohort_id`, `feature_bits`.
  - **Implementation note (Phase 1):** Tokens are currently returned as Base64-encoded JSON claims; Ed25519 signing will be introduced alongside production hardening.
- **Counters:** Redis keys `fallback:cohort:{cohort_id}:yyyy-mm-dd-hh` with TTL 90 minutes; soft limit at 0.5% triggers alert but token still issued; hard kill switch toggles `fallback_ws_paused`.
- **Privacy:** if `telemetry_opt_in=false`, control plane sets `feature_bits` to disable non-essential analytics; metrics aggregated per cohort only. Redis guardrail data stores counts only (no user identifiers).
- **Entitlement Notes:** OIDC (via Clerk) integration handled once “private beaches” feature lands; beach-rescue exposes flag `BEACH_RESCUE_DISABLE_OIDC=1` to bypass entitlement checks in dev/local environments.

## Load & Reliability Test Plan (Deferred)
- Comprehensive load testing (10k idle / 5k active targets) will be scheduled after feature completion and dev validation.
- Tooling expectations (k6 + Rust harness, chaos scenarios) documented for future execution during GA readiness.

## Review & Next Steps
- Await feedback from transport, infra, and security maintainers by 2025-10-18; track requested changes in TP-4821 / INF-3377 / SEC-2190 (current decisions incorporated here).
- Feed reviewer comments into Phase 1 kickoff brief and update this outline if scope shifts.
- Coordinate with platform team on fallback token API implementation details (Redis schema, entitlement toggles, telemetry handling).
- Defer load-testing implementation until post-development hardening; update plan when nearing GA.

## Stakeholder Outreach Log
- 2025-10-11: Sent review request to `transport-wg@`, `infra-core@`, `security@` with link to this draft and requested feedback by 2025-10-18.
- 2025-10-11: Filed tracking issues — TP-4821 (control plane fallback token minting & guardrail counters), INF-3377 (infra pipeline + registry), SEC-2190 (token signing keys review).

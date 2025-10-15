# beach-rescue Architecture Outline

Status: Phase 1 prototype – relay + telemetry online  
Last Updated: 2025-10-11  
Owner: Codex (transport planning)

## Purpose
- Capture the technical boundaries between Beach, the new `beach-rescue` WebSocket relay, and existing infrastructure.
- Describe the failover detection logic and guardrails that keep WebSocket fallback as an absolute last resort.
- Provide implementation scaffolding (contracts, repo layout, observability) ahead of Phase 2 reliability work.

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
- Deployment manifests remain staging-only until dev environment validation completes; production still targets AWS ECS Fargate with a future path to Kubernetes (documented, not yet executed).

### Phase 1 Server State (2025-10-11)
- Axum listener exposes `/ws?token=...`, `/healthz`, `/debug/stats`, and `/metrics` (Prometheus text format).
- WebSocket handshake verifies base64 token payload (JSON claims for now), enforces expiry, optional OIDC entitlement bit, and session-id match with the `ClientHello` message.
- On success, responds with `ServerHello` echoing compression + feature bits, registers the connection, and fans messages out to other peers in the same session (in-memory channel + Redis counters for active connections/messages). Writer/reader tasks are fully async; per-session UUIDs aid tracing.
- `/debug/stats` returns active session inventory plus Redis-derived totals (`fallback:metrics:*`), giving on-call quick visibility, while `/metrics` exports counters/gauges for handshakes, connection lifecycle, and fan-out metrics.
- Optional OpenTelemetry stdout exporter (`BEACH_RESCUE_OTEL_STDOUT=1`) mirrors tracing spans to aid local debugging; default builds keep tracing local only.

### Phase 2 Reliability State (2025-10-20)
- Session routing now uses a DashMap + Slab registry with bounded mpsc fan-out queues—each connection gets a buffered channel (depth 64) and flow-control drops are surfaced via `beach_rescue_flow_control_drops_total`.
- Idle recycler task runs every 30s, issuing policy close frames to sockets inactive for ≥120s and removing drained sessions (`beach_rescue_idle_pruned_total`, `beach_rescue_sessions_emptied_total`).
- Broadcast bookkeeping records delivered byte totals and per-message histograms so perf dashboards can track effective throughput per cohort.
- Same handshake contract; compression negotiation remains a pass-through hint until Phase 3 aligns client/server encoding changes.
- Beach CLI negotiator now requests fallback tokens (`POST /fallback/token`), performs the client hello/server hello exchange, and only then wraps the socket in the shared transport abstraction.

#### Telemetry Counters (Redis)
- `fallback:metrics:connections_total` – monotonically increasing count of connections accepted since boot.
- `fallback:metrics:messages_forwarded_total` – total frames relayed between peers.
- `fallback:metrics:bytes_forwarded_total` – cumulative payload bytes forwarded.
- `fallback:session:{session_id}:connections_active` – live connection count per session.
- `fallback:session:{session_id}:messages_forwarded` – frames forwarded for the session (debug/stats aggregates these).

#### Prometheus Metrics (Phase 2 Additions)
- `beach_rescue_connections_active` / `beach_rescue_sessions_active` (gauges) – real-time connection/session counts.
- `beach_rescue_messages_forwarded_total`, `beach_rescue_bytes_forwarded_total`, `beach_rescue_message_size_bytes` – delivered frames + payload histograms.
- `beach_rescue_flow_control_drops_total` – number of messages dropped due to bounded queue backpressure.
- `beach_rescue_idle_pruned_total` / `beach_rescue_sessions_emptied_total` – idle recycler actions and session drains.

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
  - Emergency feature flag in control-plane API to disable token issuance; clients degrade to WebRTC retries only. `beach-road` now exposes an environment toggle (`FALLBACK_WS_PAUSED`) that forces `/fallback/token` to return a structured 403, giving operators an immediate shutoff while longer-term flag plumbing lands. `/health` responds with `fallback_paused` so observers can confirm the switch state.
  - SRE runbook for manual draining (mark all sessions `draining`, send CLOSE frames, scale deployment to zero).
- **Alerting**
  - Prometheus alerts on high activation rate (soft breach), connection churn > 20%/5min, CPU > 70%, and handshake failure spikes; alerts page the transport on-call rotation.
  - Structured logs shipped to Loki; `transport.fallback.activated` / `guardrail_soft_breach` events feed PagerDuty + Slack. Metrics emitted as privacy-preserving aggregates (counts per cohort, no user identifiers) with opt-out respected (clients can disable telemetry, which suppresses non-essential metrics).
  - Control-plane exposes `/metrics` with `beach_fallback_token_requests_total{outcome,guardrail}`, enabling dashboards to spot kill-switch triggers and entitlement denials.
- **Rollout gates**
  - Must pass soak test (48h) in staging with synthetic clients before first prod cohort.
  - Require sign-off from security (key handling review) and transport leads (perf budgets).

## Control Plane & Token API Decisions
- **Endpoint:** `POST /v1/fallback/token` (authenticated via mTLS service account) returning `{ token, expires_at, cohort, guardrail_hint }`.
- **Inputs:** `session_id`, `cohort_id`, `client_version`, `entitlement_proof` (optional JWT from future Clerk/OIDC integration), `telemetry_opt_in`.
  - CLI already forwards overrides via `--fallback-*` flags (mirrored to `BEACH_FALLBACK_COHORT`, `BEACH_ENTITLEMENT_PROOF`, `BEACH_FALLBACK_TELEMETRY_OPT_IN`), and beach-web surfaces matching advanced controls that are stored locally and injected into the connection handshake, so Clerk proofs can slide in later without further protocol churn.
- **Validation Flow:** verify entitlement when proof provided (paid “private beaches” users); in dev or when entitlement disabled, skip OIDC checks. Ensure `fallback_ws_enabled` true. Guardrail counters stored in Redis (shared with beach-road) using hourly buckets.
- **Token Format:** Ed25519-signed CBOR payload containing `session_id`, `issued_at`, `expires_at` (<= 5 min), `cohort_id`, `feature_bits`.
  - **Implementation note (Phase 1):** Tokens are currently returned as Base64-encoded JSON claims; Ed25519 signing will be introduced alongside production hardening.
- **Counters:** Redis keys `fallback:cohort:{cohort_id}:yyyy-mm-dd-hh` with TTL 90 minutes; soft limit at 0.5% triggers alert but token still issued; hard kill switch toggles `fallback_ws_paused`.
- **Privacy:** if `telemetry_opt_in=false`, control plane sets `feature_bits` to disable non-essential analytics; metrics aggregated per cohort only. Redis guardrail data stores counts only (no user identifiers).
- **Entitlement Notes:** OIDC (via Clerk) integration handled once “private beaches” feature lands; beach-rescue exposes flag `BEACH_RESCUE_DISABLE_OIDC=1` to bypass entitlement checks in dev/local environments.
- **Client integration:** CLI requests already pass cohort/proof/telemetry overrides into the fallback token fetch; the browser wiring captures the same overrides and logs them during transport setup, pending final wiring to the fallback HTTP call once WSS support lands.

## Load & Reliability Test Plan (Deferred)
- Comprehensive load testing (10k idle / 5k active targets) will be scheduled after feature completion and dev validation.
- Tooling expectations (k6 + Rust harness, chaos scenarios) documented for future execution during GA readiness.

## Review & Next Steps
- Transport/infra/security reviewers signed off 2025-10-18; follow-ups remain tracked via TP-4821 / INF-3377 / SEC-2190.
- Harden activation controls: rely on control-plane entitlements (`fallback_ws_enabled`), document dev overrides, and add server-side kill-switch endpoints.
- Publish Prometheus metric documentation/dashboards (new flow-control/idle metrics) and wire alerts before load tests.
- Exercise the load/chaos harness against the phase-2 registry (5K active / 10K idle targets) and capture soak results ahead of Phase 3 client/server integration.

## Stakeholder Outreach Log
- 2025-10-11: Sent review request to `transport-wg@`, `infra-core@`, `security@` with link to this draft and requested feedback by 2025-10-18.
- 2025-10-11: Filed tracking issues — TP-4821 (control plane fallback token minting & guardrail counters), INF-3377 (infra pipeline + registry), SEC-2190 (token signing keys review).

## Open Source Boundary
- The `apps/beach` workspace is expected to be open sourced, but `apps/beach-rescue` (server + infra) remains closed. Only the protocol-facing `beach-rescue-client` crate is shared so public builds can serialize `ClientHello` / `ServerHello` payloads.
- Keep the client crate narrowly scoped: message structs, feature-bit definitions, and local-only helpers (e.g., ephemeral token minting) are acceptable; guardrail logic, Redis key conventions, and relay internals stay private behind the HTTP/WSS surface.
- Document in the public repo that WebSocket fallback is optional and requires the proprietary beach-rescue deployment. Provide guidance on disabling the feature flag when the service is unavailable to avoid confusing OSS adopters.

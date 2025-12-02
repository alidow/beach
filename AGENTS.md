# AGENTS Guidance

This file captures repo-wide tips for coding agents (Codex, Claude, etc.).

## Docker + env

- We no longer use `direnv`. Source `.env` (or export the required vars) before running Docker Compose. Do not prefix commands with `direnv exec`.
- Use a single session-server base: set `BEACH_SESSION_SERVER_BASE`
  (default `http://api.beach.dev:4132`). Add `127.0.0.1 api.beach.dev` to
  /etc/hosts; Compose maps `api.beach.dev` to the host gateway. Other
  variables (`BEACH_SESSION_SERVER`, `BEACH_ROAD_URL`,
  `BEACH_PUBLIC_SESSION_SERVER`, Vite/Next URLs) derive from the base; avoid
  container-local names like `beach-road:4132`.
- Do not prebuild binaries inside the dev Docker stack or test scripts. All
  agents and demos must invoke `cargo run` inside containers so code changes
  are picked up. The only exception is the remote SSH bootstrap feature of
  `apps/beach`, which requires a pre-built binary; outside of that path,
  prebuilds are forbidden.
- `apps/beach` is transport-only: it must not depend on Postgres or any DB. All
  persistence (SeaORM/Postgres, assignment tables, etc.) lives in the manager
  stack. Hosts talk over WebRTC/bus only; DB access is only from the manager or
  other backend services running on the host machine.
- When toggling managers, use `BEACH_MANAGER_IMPL=legacy|rewrite`. Inside
  docker, legacy service is `http://beach-manager:8080`; rewrite is
  `http://beach-manager-rewrite:8081`. `PRIVATE_BEACH_MANAGER_URL` should
  reflect the chosen manager.

## Auth north star

- Authenticate external requests with Clerk, authorize controller access with
  Beach Gate. Manager should accept Clerk-issued JWTs (Clerk JWKS/issuer/aud
  must be set in env) and only rely on Beach Gate for controller token
  issuance/verification after auth succeeds. If you see `jwks missing requested
  kid ... kid=ins_*`, the manager is still using Beach Gate’s JWKS for auth and
  needs its Clerk config populated.

## Peer session terminology (WebRTC attach flow)

- `host_session_id`: the long-lived public terminal session the host registers.
- `peer_session_id`: the per-peer attachment session created via `POST /peer-sessions/attach` before any WebRTC offer/answer. Browser/manager must call attach first, then use `peer_session_id` for `/peer-sessions/:id/webrtc/offer|answer`.
- Offers/answers no longer 404 for “unknown session”; attach is required. If polled too early, expect a retryable status (e.g., 409 + Retry-After).

## Transport note

- The legacy “fast-path” transport is gone; everything is unified WebRTC P2P (or HTTP fallback when needed). Treat any “fast-path” references in old logs/docs as stale.
- Manager ↔ host and browser ↔ host now speak the same single Beach WebRTC transport as any other client; there is no special controller channel or peer. If you see code branching for “fast path” peers/labels/hints, assume it should be deleted or folded into the primary transport.
- The CLI host (`apps/beach`) should only understand “host” and “client” roles. Any awareness of Beach Manager, controllers, or mgr-actions state belongs in the harness (`crates/beach-buggy`). Hosts should attach the unified bridge and let beach-buggy consume/produce controller/state/health frames over the unified channel; do not add controller-specific data channel handling to the host.
- Manager joins host sessions as a normal client via Beach Road signaling (wss) and talks to hosts only over the unified WebRTC channel. Do not add HTTP-based cache/state sync or extra data channels (e.g., `mgr-actions`/`mgr-acks`); everything must ride the unified transport.
- Cache/state syncing between host ↔ manager or host ↔ browser must never use HTTP. Unified WebRTC + wss signaling only—no host-side HTTP pollers or publishers for cache/state.

## Private Beach: Public Host Publish Token Plan

### Background
- Today an ordinary “public” Beach host can be invited into a private beach by running `beach host…` with zero prior knowledge of that beach.
- When the private beach manager accepts the host, it sends an auto-attach hint that currently contains:
  - The controller lease token (used for `queue_actions`, `poll_actions`, etc.).
  - Session metadata (manager URL, attach code, etc.).
- **Missing:** Any credential that authorises the host to POST state snapshots / health to Manager. The CLI assumes it already has a Beach Auth access token (`beach auth login`), so when we enforced auth for `/sessions/:id/state` the periodic idle snapshots stopped for public hosts.
- Result: the agent never receives fresh `terminal_full` frames, so it keeps the paddles in `stale` and Pong never starts.

### Problem Statement
> We must preserve the UX contract: public beach hosts must be attachable to a private beach without pre-existing logins or knowledge, yet Manager must still require authentication for state/health endpoints.

### Acceptance Criteria
1. A host added to a private beach receives *all* credentials it needs (controller token + new publish token) from Manager at attach time; no `beach auth login` required.
2. Host CLI transparently uses the new publish token for:
   - `POST /sessions/:id/state`
   - `POST /sessions/:id/health`
   - Periodic idle snapshots / health pings.
3. Manager verifies the publish token (signature + session binding + expiry) even if `AUTH_BYPASS=1`.
4. Existing flows (CLI already logged-in with Beach Auth) keep working; i.e. code falls back to stored bearer if present.
5. Logs clearly indicate which auth path was used (publish token vs Beach Auth).

### Proposed Solution Overview
- Mint a *session-scoped publish token* whenever Manager registers a session (or when it issues the auto-attach hint). This can be:
  - A JWT signed by Manager with claims: `{ "sid": "<session_id>", "exp": <ts>, "scopes": ["state_publish"] }`
  - Or a random bearer stored in Redis with metadata (simpler but requires persistence).
- Include the token + expiry in `transport_hints.idle_snapshot.publish_token`.
- Update the host CLI:
  - When parsing `transport_hints`, prefer `publish_token` if present.
  - Use it as the bearer for `state` / `health` POSTs and idle snapshots.
  - Fall back to Beach Auth token if no publish token was provided.
- Update Manager routes:
  - `POST /sessions/:id/state` and `/sessions/:id/health` accept either:
    1. Beach Auth bearer (current behaviour).
    2. Signed publish token (validate signature, `sid`, `exp`).
  - Add middleware/helper to decode and validate publish tokens.
- Security: tokens are short-lived (e.g. 30 min) and scoped to a single session id to limit blast radius.

### Implementation Plan / Tasks
1. [x] **Token creation**
   - [x] Extend `AppState::register_session` to generate a publish token and add it to `transport_hints.idle_snapshot.publish_token`.
   - [x] Store expiry alongside token claims (JWT `exp`); re-issue on next register/attach.
2. [x] **Manager verification**
   - [x] Add module `publish_token.rs` responsible for signing/verifying JWTs (HMAC HS256 with secret `PUBLISH_TOKEN_SECRET`, 30‑min TTL).
   - [x] Accept either a publish token or Beach Auth bearer in `push_state`/`signal_health` via a new `authorize_publish` helper. Publish tokens are always strictly verified even when `AUTH_BYPASS=1`.
3. [x] **CLI changes**
   - [x] In `host.rs`, parse `idle_snapshot.publish_token` from transport hints and prefer it over Beach Auth for HTTP publishing.
   - [x] Idle snapshot worker uses the chosen bearer; continues to fall back to Beach Auth if no publish token is present.
   - [ ] Optionally parse the token from late `manager_handshake` payload to update a running worker (not required for initial rollout).
4. [x] **Hint schema/documentation**
   - [x] Update `docs/private-beach/public-host-auto-attach.md` with the new `publish_token` field.
5. [~] **Testing**
   - [x] Unit tests for token sign/verify (`apps/beach-manager/src/publish_token.rs`).
   - [~] Integration: manual path described below due to unrelated test harness compile issues.
   - [~] Regression: documented verification steps for Beach Auth fallback.

### Open Questions / Follow-ups
- Do we need token refresh? (Probably not if attach happens shortly before play; but we can re-issue on reconnect if expired.)
- Storage choice: JWT (stateless) vs DB/Redis (stateful) – lean toward JWT for simplicity.

### Tracking
- Status: implemented in code; compile/test in local harness may require fixing unrelated webrtc/test deps. See Testing section.

### Testing
- Token unit tests: `cargo test -p beach-manager --lib publish_token` (runs `sign_and_verify_round_trip`, `sid_mismatch_rejected`).
- Manual integration outline (works with Docker stack):
  - Start Manager with `AUTH_BYPASS=1` and set `PUBLISH_TOKEN_SECRET`.
  - Launch a public host without `beach auth login` and with idle snapshots enabled.
  - Observe host logs: `idle snapshot worker enabled` and idle publish succeeds using `publish_token`.
  - Observe Manager logs for `push_state received auth_path=publish_token` and `signal_health accepted auth_path=publish_token`.
  - Log in with Beach Auth and repeat; observe `auth_path=bearer` and identical behaviour.

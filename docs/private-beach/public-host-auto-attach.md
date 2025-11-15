# Public Host Auto-Attach Plan

## Problem

The CLI host only calls `attach-by-code` when both `PRIVATE_BEACH_ID` and `PRIVATE_BEACH_SESSION_PASSCODE` are set. Public hosts launched from **docs/helpful-commands/pong.txt** do not export those env vars, so they never attach to the private beach, their `/controller/actions/poll` requests return 404, and controller queues fill to 500.

Goal: public hosts must auto-attach without any manual environment variables. All information required to attach (private beach id, attach token, manager URL, and a scoped manager credential) must travel through the existing session/bootstrap handshakes so the host can pick it up automatically.

## Desired UX

1. Private Beach Manager records which private beach a session belongs to and issues an attach token (passcode or single-use key).
2. When the host joins via `beach … host --session-server …`, the bootstrap payload includes a new `controller_auto_attach` hint with:
   ```json
   {
     "private_beach_id": "<uuid>",
     "attach_code": "<string>",
     "manager_url": "http://localhost:8080",
     "issued_at": "<timestamp>",
     "expires_at": "<timestamp>"   // optional
   }
   ```
   Additionally, when idle snapshots are enabled, the `idle_snapshot` hint now includes a per-session `publish_token` so public hosts can publish state/health without Beach Auth logins:
   ```json
   {
     "idle_snapshot": {
       "interval_ms": 1000,
       "mode": "terminal_full",
       "publish_token": { "token": "<jwt>", "expires_at_ms": 1731020400000 }
     }
   }
   ```
3. The host inspects the hint first; if present it immediately POSTs `/private-beaches/<id>/sessions/attach-by-code` before starting the controller poller/fast_path. Env vars remain as fallback only. When the dashboard issues a `manager_handshake`, the payload mirrors the same `controller_auto_attach` block so late-emitted hints (e.g., after a UI attach) reach already-running hosts without requiring a restart.
4. Logs show whether the host attached via metadata or envs, making future diagnosis trivial.

## Credential Model (Tokens)

Public hosts should **never** need to preconfigure `PB_MANAGER_TOKEN` or any user-scoped credential. Instead, Beach Manager issues narrowly scoped tokens over trusted channels:

- **User token** (browser / CLI):
  - Scope: full account capabilities (e.g., `pb:beaches.*`, `pb:sessions.*`).
  - Used by the Private Beach UI and `beach` CLI to create beaches, attach/detach sessions, and manage agents.
  - Never forwarded to hosts or agents.

- **Controller token** (per child session):
  - Minted when a controller lease is acquired for a session.
  - Carried today in `controller_token` fields:
    - Manager → agent bridge (HTTP `sessions/{id}/actions`).
    - Manager → host fast-path (`mgr-actions` channel).
  - Scope: drive this specific child session’s controller queue; not accepted on arbitrary Private Beach APIs.

- **Publish / harness token** (per-session credential):
  - Minted by Beach Manager’s `PublishTokenManager` whenever a session is registered or attached to a private beach.
  - Intended uses:
    - `POST /private-beaches/{id}/sessions/attach-by-code` for the specific `{private_beach_id, session_id}` pair.
    - State/health publishing for that session (`/sessions/:id/state`, `/sessions/:id/health`, idle snapshots).
  - Scope:
    - Bound to a single `session_id` (`sid` claim).
    - Short-lived (e.g., ~30 minutes), renewable when Manager refreshes idle snapshot hints.
    - Limited to a narrow set of endpoints (attach-by-code, state/health, maybe per-session layout reads), never global `pb:beaches.*`.

### How Tokens Flow

1. **Attach in UI**
   - A user with a full user token attaches a public session to a private beach.
   - Manager:
     - Records the relationship.
     - Issues a controller lease and `controller_token` for the child session.
     - Mints / refreshes a **publish/harness token** scoped to that `session_id`.

2. **Manager → host via Beach Road**
   - Manager sends a `manager_handshake` control message down the session’s control channel through Beach Road.
   - Payload includes:
     - `controller_token` (for controller I/O).
     - `controller_auto_attach` hint (private beach id, attach code, manager URL, timing).
     - Idle snapshot/publish hints carrying the per-session publish token:
       - `idlePublishToken` (shim used by the CLI host) and/or
       - `idle_snapshot.publish_token` (the canonical hint stored in `transport_hints`).

3. **Host → manager using publish/harness token**
   - The CLI host’s auto-attach logic:
     - Prefers the publish/harness token from the handshake for manager HTTP calls.
     - Falls back to legacy user tokens only when no publish token is present (for backwards compatibility and older managers).
   - The host calls:
     - `POST /private-beaches/{id}/sessions/attach-by-code` with `Authorization: Bearer <publish_token>`.
   - Manager validates:
     - Token signature and expiry via `PublishTokenManager`.
     - `sid` (session id) in the token matches the route `session_id`.
     - Treats callers authenticated this way as a “harness” (no account id), limited to attach + publish operations for that session.
     - Attach code matches the session’s join/attach code.

4. **Controller path**
   - Once attached:
     - Manager’s controller forwarder uses `controller_token` to write into the child’s `actions` queue and stream acks.
     - Host uses the same `controller_token` (and/or fast-path channel) to receive actions and send acks.
   - Neither side needs the user token at this point; they operate with session-scoped credentials only.

## Implementation Steps

### 1. Manager: capture and expose auto-attach metadata

1. Extend `SessionRecord` (`apps/beach-manager/src/state.rs`) to store the private beach id + attach code whenever `attach_by_code` succeeds. We already know the `private_beach_id`, `session_id`, `join_code`, and `public_manager_url` in that handler.
2. Add a helper that builds a `ControllerAutoAttachHint` struct and injects it into `session.transport_hints["controller_auto_attach"]`. This hint should include:
   - `private_beach_id`
   - `attach_code` (use the existing join code or mint a derivative such as a short-lived token)
   - `manager_url` (prefer `public_manager_url`, fall back to local `http://localhost:8080`)
   - Optional `expires_at` (if codes expire) so clients can log useful expiry errors.
3. Make sure the hint persists through both in-memory + Postgres backends (update DB serialization if necessary).
4. Confirm the session payload returned by Road → CLI (`SessionHandle::session()`, `Session::transport_hints`) now includes the new block when a session belongs to a private beach.
5. Tests:
   - Unit test the hint builder given fake session records.
   - Integration test attach route → session fetch, asserting the hint exists.

### 2. Manager: reuse publish tokens as harness tokens

1. Reuse `PublishTokenManager` (`apps/beach-manager/src/publish_token.rs`) as the harness token issuer. It already mints per-session JWTs with:
   - `sid = session_id`.
   - `scp = ["pb:sessions.write", "pb:harness.publish"]`.
2. Ensure every attached session has an up-to-date publish token:
   - `AppState::attach_by_code` already calls `refresh_idle_publish_token_hint` after attach; that keeps the per-session token fresh.
3. Confirm `send_manager_handshake` includes an `idle_publish_token` hint:
   - Payload contains:
     ```json
     "idle_publish_token": { "token": "<jwt>", "expires_at_ms": 1731020400000, "scopes": ["pb:sessions.write","pb:harness.publish"] }
     ```
   - This is the same token the host uses for idle snapshots/health and now for attach-by-code.
4. Update `/private-beaches/:private_beach_id/sessions/attach-by-code` to accept either:
   - A normal Beach Auth bearer with `pb:sessions.write`, or
   - A publish token where `sid == session_id`. In the latter case, treat the requester as “harness” (no account id).
5. Tests:
   - Verify that publish tokens are still accepted on publish endpoints.
   - Add an integration test: call `attach-by-code` with a publish token and valid code and assert the session attaches successfully.

### 3. Host: consume hints and publish tokens before env vars

1. During bootstrap (`apps/beach/src/server/terminal/host.rs`), we already have a `SessionHandle` whose metadata contains `transport_hints`. Keep parsing `controller_auto_attach` as before.
2. When a `manager_handshake` arrives, the host:
   - Extracts `idle_publish_token` (if present) and stores it in `IdleSnapshotController`.
   - Applies any idle snapshot/health intervals from the same payload.
3. Update handshake handling so the same per-session publish token is available to the auto-attach logic:
   - Pre-extract the publish token from `idle_publish_token` / `idle_snapshot` hints inside the handshake payload.
   - Pass this token into `trigger_auto_attach` as an optional bearer override.
4. Update `maybe_auto_attach_session` to:
   - Prefer the bearer override when present (publish token from handshake).
   - Fall back to `resolve_manager_bearer(manager_url)` (legacy user token) only when no publish token is available.
   - Keep existing logging and backoff behavior.
5. Ensure the attach POST uses the hint’s attach code exactly as Manager expects.
6. Tests:
   - Unit-test `maybe_auto_attach_session` to prove it uses the bearer override when supplied.
   - Run `cargo fmt` + `cargo test -p beach`.

### 4. Verification

1. Rebuild `beach-manager` and `beach` CLI, restart docker stack.
2. Launch Pong hosts without any `PRIVATE_BEACH_*` env vars. Host logs must show `auto-attached session via manager (source=metadata)` and no longer spam `skipping auto-attach`.
3. Confirm Manager logs show controller forwarders connected and action queues stay below limit. `~/beach-debug/beach-host-*.log` should no longer contain `receive_actions … 404`.
4. Run full Pong demo (LHS/RHS hosts + agent) and verify paddles move.

### 5. Documentation / Cleanup

1. Update `docs/helpful-commands/pong.txt` to remove the env vars from the troubleshooting tips (keep a note about manual override).
2. Add a note to the private beach runbooks that attach-by-code is now automatic with per-session harness tokens.
3. Ensure secrets (attach codes) are redacted from logs where necessary.

## Open Questions / Constraints

- If attach codes expire quickly, consider minting a short-lived token when emitting the hint (e.g., `manager.issue_attach_token(session_id)`), so sharing the real passcode isn’t required.
- Verify the hint only appears for sessions tied to a private beach; public-only sessions shouldn’t leak stale codes.
- Make sure multiple hosts joining the same private beach receive their own attach tokens (or reuse the same passcode if acceptable).

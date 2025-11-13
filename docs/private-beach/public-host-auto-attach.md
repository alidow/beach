# Public Host Auto-Attach Plan

## Problem

The CLI host only calls `attach-by-code` when both `PRIVATE_BEACH_ID` and `PRIVATE_BEACH_SESSION_PASSCODE` are set. Public hosts launched from **docs/helpful-commands/pong.txt** do not export those env vars, so they never attach to the private beach, their `/controller/actions/poll` requests return 404, and controller queues fill to 500.

Goal: public hosts must auto-attach without any manual environment variables. All information required to attach (private beach id, attach token, manager URL) must travel through the existing session bootstrap metadata so the host can pick it up automatically.

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
3. The host inspects the hint first; if present it immediately POSTs `/private-beaches/<id>/sessions/attach-by-code` before starting the controller poller/fast_path. Env vars remain as fallback only. When the dashboard issues a `manager_handshake`, the payload now mirrors the same `controller_auto_attach` block so late-emitted hints (e.g., after a UI attach) reach already-running hosts without requiring a restart.
4. Logs show whether the host attached via metadata or envs, making future diagnosis trivial.

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

### 2. Host: consume hints before env vars

1. During bootstrap (`apps/beach/src/server/terminal/host.rs`), we already have a `SessionHandle` whose metadata contains `transport_hints`. Add a parser that extracts `controller_auto_attach` into an `AutoAttachContext`.
2. Update `maybe_auto_attach_session`:
   - Accept the parsed hint plus legacy env values.
   - Preference order: metadata hint → env vars → nothing.
   - Extract `manager_url` from the hint; otherwise use `PRIVATE_BEACH_MANAGER_URL` or default `http://localhost:8080`.
   - When using metadata, log `auto-attach via metadata`.
   - Preserve existing logging for env fallback/missing fields.
3. Ensure the attach POST uses the hint’s attach code exactly as Manager expects.
4. Tests:
   - Unit-test the parser with valid/invalid hint JSON.
   - Unit-test `maybe_auto_attach_session` to prove it issues a request when metadata exists (use `reqwest::Client` mock / feature flag).
5. Run `cargo fmt` + `cargo test -p beach`.

### 3. Verification

1. Rebuild `beach-manager` and `beach` CLI, restart docker stack.
2. Launch Pong hosts without any `PRIVATE_BEACH_*` env vars. Host logs must show `auto-attached session via manager (source=metadata)` and no longer spam `skipping auto-attach`.
3. Confirm Manager logs show controller forwarders connected and action queues stay below limit. `~/beach-debug/beach-host-*.log` should no longer contain `receive_actions … 404`.
4. Run full Pong demo (LHS/RHS hosts + agent) and verify paddles move.

### 4. Documentation / Cleanup

1. Update `docs/helpful-commands/pong.txt` to remove the env vars from the troubleshooting tips (keep a note about manual override).
2. Add a note to the private beach runbooks that attach-by-code is now automatic with per-session tokens.
3. Ensure secrets (attach codes) are redacted from logs where necessary.

## Open Questions / Constraints

- If attach codes expire quickly, consider minting a short-lived token when emitting the hint (e.g., `manager.issue_attach_token(session_id)`), so sharing the real passcode isn’t required.
- Verify the hint only appears for sessions tied to a private beach; public-only sessions shouldn’t leak stale codes.
- Make sure multiple hosts joining the same private beach receive their own attach tokens (or reuse the same passcode if acceptable).

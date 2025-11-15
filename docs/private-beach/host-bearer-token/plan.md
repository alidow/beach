# Plan: Harness Tokens for Public Hosts (Historical Design + Final Shape)

## Goal
Ensure public hosts (CLI `beach host …`) never need to set `PB_MANAGER_TOKEN` or run `beach login`. Hosts should receive a **session-scoped harness token** over the existing control channel and use it to:

- Call `/private-beaches/{id}/sessions/attach-by-code` exactly once.
- Publish idle snapshots / health without any Beach Auth login.

Originally this plan proposed adding a dedicated `bearer` field to `ControllerAutoAttachHint`. In practice we achieved the goal by **reusing the per-session publish token** minted by `PublishTokenManager` and exposing it via the `idlePublishToken` / `idle_snapshot.publish_token` hints. The host now treats that publish token as its harness credential for both state/health and `attach-by-code`.

This file is kept as a **design record**; do not reintroduce a separate `bearer` field unless we have a compelling reason. New work should align with the publish-token based design described in:

- `docs/private-beach/public-host-publish-token-plan.md`
- `docs/private-beach/public-host-auto-attach.md`

## High-Level Steps (as implemented)
1. **Mint a scoped publish/harness token** whenever Manager registers or attaches a session.
2. **Expose the token in transport hints** (`idle_snapshot.publish_token` and the `idlePublishToken` shim used in `manager_handshake`).
3. **Update host auto-attach logic** to prefer the handshake-provided publish token as its bearer for `attach-by-code`.
4. **Use the same token for idle snapshots and health** so public hosts never need Beach Auth.
5. **Add instrumentation/tests** so we can audit which auth path was used.
6. **Document rollout/compatibility** so older hosts (without publish tokens) keep working and public launch snippets never mention `PB_*` secrets.

## Detailed Work (historical sketch vs. final implementation)

The original version of this plan suggested adding a `bearer: Option<String>` field to `ControllerAutoAttachHint`. Instead, we:

### 1. Harness token = publish token
- Added `apps/beach-manager/src/publish_token.rs` and taught `AppState::register_session` to mint a per-session publish token.
- Included that token and expiry in `transport_hints.idle_snapshot.publish_token`.
- In `send_manager_handshake`, we surface the same token under the `idlePublishToken` key so already-running hosts can pick it up without re-registering.

### 2. Manager verification
- `authorize_publish` accepts either:
  - A Beach Auth bearer, or
  - A signed publish token whose `sid` and `exp` claims must match the session.
- Even when `AUTH_BYPASS=1`, publish tokens are strictly validated so public hosts cannot spoof session IDs.

### 3. Host auto-attach changes
- `apps/beach/src/server/terminal/host.rs` now:
  - Parses `idle_snapshot.publish_token` from bootstrap `transport_hints`.
  - Parses `idlePublishToken` and nested `idle_snapshot.publish_token` from `manager_handshake` payloads.
  - Stores the resulting token in the idle snapshot worker **and** passes a clone into `trigger_auto_attach` as `bearer_override`.
- `maybe_auto_attach_session`:
  - Prefers the `bearer_override` (publish token from handshake) when present.
  - Falls back to `resolve_manager_bearer(manager_url)` only when no publish token is available (older manager builds).
  - Logs whether an override was used without ever printing the token contents.

### 4. Instrumentation and tests
- Manager logs include structured fields indicating which auth path was used when accepting state/health.
- Unit tests in `publish_token.rs` cover basic sign/verify and sid mismatch cases.
- Manual integration steps are captured in `docs/private-beach/public-host-publish-token-plan.md` and `docs/private-beach/public-host-auto-attach.md`.

### 5. Rollout / compatibility
- Because publish tokens are optional hints, older hosts still work when launched with `beach auth login` or `PB_MANAGER_TOKEN`, but:
  - The **preferred** flow is now “no PB secrets at all; rely on publish tokens from Manager”.
  - `docs/helpful-commands/pong.txt` only mentions `PRIVATE_BEACH_MANAGER_URL` as a convenience; it does **not** require PB bearer tokens.
- AGENTS for the rewrite stack explicitly calls out that public sessions must not be provisioned with Private Beach bearer tokens. Our current implementation complies: hosts see only session-scoped publish tokens, never account-scoped credentials.

## Acceptance Criteria (met)
- Running the CLI hosts without `PB_MANAGER_TOKEN` succeeds: logs show `auto-attached via handshake` with no `bearer token unavailable` messages once the publish-token path is wired.
- Manager logs confirm publish token issuance and usage during handshake and state/health pushes.
- An end-to-end Pong showcase can complete without manual PB token exports once independent transport/auth issues (JWKS mismatch, fast-path ICE config, agent readiness) are resolved.

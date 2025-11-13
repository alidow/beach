# Public Host Auto-Attach Handshake Plan

## Problem
Public CLI hosts still require `PRIVATE_BEACH_*` env vars to call `/private-beaches/:id/sessions/attach-by-code`. When a tile adds a public session to a private beach the UI calls `attach_by_code`, and Beach Manager sends a “manager handshake” control message down the existing host connection, but the host ignores it. As a result:

1. Hosts keep polling `/controller/actions/poll` anonymously and receive 404s.
2. Manager never sees the host as “attached,” so controller forwarders / fast_path never engage.
3. The Pong agent sits in “waiting for players (transport: pending)” forever.

We need the host to consume the handshake, learn the private beach metadata, and attach itself automatically—no manual env vars.

## Proposed Solution

### 1. Manager: Emit structured handshake payload
The current handshake (`send_manager_handshake` in `apps/beach-manager/src/state.rs`) already pushes a `manager_handshake` control frame containing:
- `private_beach_id`
- `manager_url`
- `controller_token` (auto-handshake lease)
- `controller_auto_attach` hint `{private_beach_id, attach_code, manager_url}`

We’ll standardize the payload schema (JSON object) and document it.

### 2. Host: Control-message listener
Add a listener to the CLI host (`apps/beach/src/server/terminal/host.rs`) that subscribes to control frames (the same channel used for `beach:status:*`). When it receives a `manager_handshake` payload:

1. Parse the JSON, extract `controller_auto_attach`.
2. If the host is already attached (state flag), ignore; otherwise proceed.
3. Spawn an async task to POST `/private-beaches/{id}/sessions/attach-by-code` using:
   - `manager_url` (from handshake or existing env fallback)
   - `session_id` (current host session)
   - `code` (attach_code from handshake)
   - Authorization: reuse `PRIVATE_BEACH_MANAGER_TOKEN` or fetch from the handshake (if we add a token field).
4. On success, log `auto-attached via handshake` and mark the host as “manager-attached”.
5. On failure (HTTP error), log `handshake auto-attach failed` and retry with exponential backoff (bounded).

### 3. Track attach state
Introduce a small state struct (e.g., `ControllerAttachState`) shared between the host control listener and the controller action consumer:
- Fields: `attached` (bool), `last_attempt`, `failure_count`.
- Expose `await_manager_attach()` future for the controller action consumer to wait on before starting the HTTP poller or fast_path channel.

### 4. Fast-path coordination
Once attach completes:
1. Notify the controller action consumer to unpause (resume polling or fast_path).
2. When the `mgr-actions` channel appears, the host already knows it belongs to the private beach and can safely pause HTTP polling.

### 5. Backward compatibility
- If handshake never arrives (older manager), fall back to the existing env-based behavior.
- If `controller_auto_attach` block is missing, log a warning and keep retrying when future handshakes arrive.

### 6. Tests / Validation
1. Unit test: simulate a `manager_handshake` control frame and assert the host POSTs attach-by-code.
2. Integration test: start a fake manager emitting the handshake; confirm the host attaches and stops receiving 404s.
3. Log assertions: `controller.actions.fast_path` and `controller.actions.fast_path.apply` should appear once attach completes.

### 7. Observability
- New host logs:
  - `controller.handshake.received` (info)
  - `controller.handshake.attach.start/success/failure`
- Manager logs: continue emitting handshake events so debugging is easier.

## Implementation Checklist
- [ ] Define handshake payload schema and document it (struct/serde).
- [ ] Add control-message subscriber in host to handle `manager_handshake`.
- [ ] Implement auto-attach POST + retry/backoff.
- [ ] Gate controller action consumer start-up on attach completion.
- [ ] Add tests + logging described above.

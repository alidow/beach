# Auto Handshake Delivery Plan

Goal: allow public Beach hosts to obtain Beach Manager controller credentials automatically once they are added as tiles, without requiring `PRIVATE_BEACH_*` env vars.

## Overview

1. **Beach Manager** exposes a signed handshake API that verifies `{session_id, passcode}`, issues/renews controller leases, and returns the info a host needs to start draining actions.
2. **Beach Road** becomes the transport for “control” payloads (handshakes, acks, detaches). It broadcasts manager-issued messages to the appropriate host via the existing WebRTC/WebSocket signaling channel.
3. **Beach host (beach CLI)** listens for those control payloads, auto-attaches to the manager when a handshake arrives, and maintains the controller action consumer lifecycle (renew leases, shutdown on detach).
4. **Private Beach UI/runtime** calls the handshake endpoint whenever a tile is wired to a public session, pushes the payload through Beach Road, waits for an ACK, and handles reconnections/detaches.

## Detailed Steps

### 1. Beach Manager: Controller Handshake API
- Add `POST /sessions/:session_id/controller-handshake`:
  - Body: `{ passcode: string, requester_private_beach_id?: string }`.
  - Validates the passcode via existing Road verification (`sessions/:id/verify-code`), ensures the session belongs to the target private beach, and acquires/renews a controller lease (reuse `State::acquire_controller_lease`).
  - Response payload: `{ private_beach_id, manager_url, controller_token, lease_expires_at_ms, stale_session_idle_secs, viewer_health_interval_secs }`.
  - Emits a `private_beach.sessions` log (`handshake_issued`) and a `controller_event` for auditing.
- Add `DELETE /sessions/:session_id/controller-handshake` to revoke leases on detach.
- Extend state tests (`apps/beach-manager/tests/postgres_integration.rs`) to cover:
  - Happy path: verifies code, issues lease, returns payload.
  - Invalid passcode / foreign session (expect 403/404).
  - Revocation path.
- Ensure metrics capture handshake issuance and failures.

### 2. Beach Road: Control Plane Channel
- Extend signaling layer (`apps/beach-road/src/websocket.rs`):
  - Introduce a lightweight control message type, e.g. `{ "kind": "manager_handshake", payload: { … } }`.
  - Allow private-beach to post control messages via a new HTTP endpoint or reuse the existing storage layer (e.g., `POST /sessions/:id/control`).
  - Persist recent control payloads per session until ACKed (so reconnecting hosts can catch up).
- On host connect, Beach Road streams any pending control messages down the same WebSocket channel used for transport negotiation.
- Accept ACK/NACK messages from hosts (e.g., `{ "kind": "manager_handshake_ack", controller_session_id, expires_at_ms }`) and forward them to private-beach for UI state.

### 3. Beach Host: Dynamic Manager Consumer
- Refactor `apps/beach/src/server/terminal/host.rs`:
  - Introduce a “control listener” that waits for `manager_handshake` control payloads delivered via beach-road (the CLI already maintains that session connection).
  - On receipt:
    - Run `maybe_auto_attach_session` with the supplied `{private_beach_id, controller_token}`—remove the env var dependency.
    - Start/renew the controller action consumer using the received manager URL/token.
    - Log structured events (`controller.actions` target) for handshake receipt, attach success/failure, and lease expiry.
  - Support legacy env-mode as fallback (if no handshake payload arrives, keep current behavior for scripts/tests).
  - Implement lease renewals before `lease_expires_at_ms` (use the manager API or reuse `controller/lease` endpoint).
  - Handle `manager_detach` control messages by shutting down the consumer gracefully.
- Add unit/integration tests that simulate handshake reception and verify the host begins draining controller actions without env vars.

### 4. Private Beach Runtime / UI
- When a tile connects to a public session:
  - Call the new manager handshake endpoint with `{session_id, passcode}`.
  - On success, `POST` the returned payload to Beach Road’s control endpoint for that session.
  - Show “waiting for host ACK” state until a `manager_handshake_ack` arrives (timeout / retry if needed).
- On tile detach or session disconnect:
  - Send `manager_detach` control message so the host can tear down the consumer and release the lease.
- Update server-side logging (e.g., `private_beach.sessions` target) to include handshake status transitions for observability.

## Testing & Verification
- End-to-end flow:
  1. Launch a public session via `beach host` with no `PRIVATE_BEACH_*` env vars.
  2. Attach it as a tile; the UI should drive the handshake and within seconds host logs should show `manager handshake received` and controller actions being applied.
  3. Launch the Pong agent; verify controller queues stay below 500 and Pong plays.
- Regression tests:
  - Manager unit/integration tests for handshake API.
  - Beach-road tests for control message delivery/ack.
  - Host integration test (maybe via smoke test harness) to ensure handshake triggers auto-attach.
  - Private-beach UI tests (Playwright/unit) for handshake ACK workflow.

## Rollout Notes
- Keep legacy env-mode path in the host until we confirm all tile flows have migrated.
- Add feature flag/env guard while dogfooding.
- Update docs (`docs/helpful-commands/pong.txt`, new troubleshooting sections) once the new flow is confirmed.

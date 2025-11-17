# Controller Handshake Renewal – Server Fix Plan

## Problem statement

Viewer tiles currently call `POST /sessions/{id}/controller-handshake` on a timer to keep controller leases alive. Each call:

1. Replays the attach-by-code pathway on Beach Manager.
2. Issues a brand-new `handshake_id`, invalidating the previous `/webrtc/answer` URL.
3. Requires the client to send a `manager_handshake` control message so hosts pick up the new credentials.

While the intention was to refresh the lease, the side effect is that every renewal tears down the existing fast-path link between Beach Manager and the CLI hosts. The manager log is flooded with `fast-path … not established; continuing with HTTP fallback` warnings while it attempts to stand up the new mgr-* channels, resulting in 5–20 s of unavailability per renewal.

Client-side mitigations can delay or suppress these renewals, but the root cause lives on the server: **renewing a controller lease should not require discarding a healthy fast-path transport**.

## Long-term fix (server responsibilities)

1. **Introduce a renewal pathway that extends the controller lease without forcing a new handshake.**
   - Option A: add `POST /sessions/{id}/controller-handshake/renew` that returns the updated lease TTL but explicitly preserves the currently active fast-path transport.
   - Option B: detect when the existing fast-path is healthy and treat repeated calls to the existing handshake endpoint as a pure lease refresh (no new `handshake_id`, no host notifications).
2. **Keep the previous handshake valid until a replacement transport is confirmed.**
   - If a client truly needs a new fast-path (e.g., because the old transport died), only revoke the current one after the new mgr-* channels are ready.
3. **Expose telemetry so the client can tell whether the server performed a refresh or a full renegotiation.**
   - This lets the dashboard decide whether it needs to send another `manager_handshake` control message.
4. **Update documentation/tests to cover the new renewal semantics.**

## Acceptance criteria

- Repeated lease renewals (every 15–20 s) no longer cause Beach Manager to emit fast-path fallback warnings when the existing fast-path transport is healthy.
- CLI host logs should remain on “fast path controller channel ready … http action poller paused (fast path active)” without flipping back to HTTP.
- Existing clients that still call `issueControllerHandshake` continue to work; they either hit the new renew path or receive an explicit response indicating that no transport churn is required.
- Test coverage includes:
  - Unit tests for the renewal endpoint.
  - Integration test showing a renewal does not invalidate an active fast-path transport.
  - Regression test proving that a genuine transport failure still triggers a full handshake.

## Implementation notes

- `POST /sessions/:id/controller-handshake` now inspects the fast-path registry and only dispatches a new manager handshake when the existing fast-path channel is missing or unhealthy. Healthy channels simply extend their lease in-place.
- The response payload now includes `handshake_kind` (`"refresh"` or `"renegotiate"`). Tiles can treat `"refresh"` as a pure lease extension and skip the `manager_handshake` control message. `"Renegotiate"` indicates that the manager issued new credentials and hosts should receive another handshake.
- The renewal logic keeps the previous fast-path data channel online until the replacement is confirmed, so mgr-* channels no longer flap during periodic lease renewals.

## References

- `temp/pong.log` (2025‑11‑17T16:24–16:26Z) – repeated `handshake:start` and `fast-path:success` logs every ~15 s.
- `logs/beach-manager/beach-manager.log` (lines 3 4xx–3 9xx, 13 2xx, 57 7xx) – controller repeatedly falling back to HTTP fast-path during renewals.

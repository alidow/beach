# Restoring generic fast-path for any terminal session

Context and plan for enabling WebRTC/fast-path for **any** Beach terminal session (public or private) when attached as an application tile to a private beach (no preseeded harness/layout required).

## Current state (problem)
- Manager no longer advertises `fast_path_webrtc` in the handshake; default transport hints only carry `extensions` (fastpath namespace).
- The legacy fast-path server is effectively removed: `apps/beach-manager/src/fastpath.rs` is a stub, and its routes are not registered in the router.
- When attaching a public host session to a private beach, manager logs `manager did not advertise fast-path endpoints; reverting to HTTP`. Browser tries WebRTC fast-path with passcode, but host and viewer derive different keys → `authentication tag mismatch`.
- Harness-configured beaches still work (unified fast-path via extensions), but generic/public sessions do not.

## Goal
Re-enable fast-path for generic sessions: any Beach terminal session attached to a private beach should get working WebRTC fast-path to both manager and browser, without requiring harness/layout/controller pairings.

## Execution plan
1) **Restore manager fast-path server**
   - Replace the stubbed `fastpath` module with a working implementation (WebRTC offer/answer/ICE) and re-route `/fastpath/sessions/:id/*` endpoints in `routes/mod.rs`.
   - Ensure auth: accept publish token for the session or a bearer with `pb:harness.publish` (current behavior in `routes/fastpath.rs`).

2) **Advertise default fast-path hint for all sessions**
   - In handshake construction (`send_manager_handshake`), if no harness-specific fast-path hint exists, inject a default `fast_path_webrtc` pointing to the restored endpoints:
     - `offer_path`: `/fastpath/sessions/{session_id}/offer`
     - `ice_path`: `/fastpath/sessions/{session_id}/ice`
     - Channels: actions/acks/state labels defaulted (use same labels as existing fast-path channels or sensible defaults).
   - Keep existing `extensions` hint so unified transports remain unchanged.

3) **Buggy acceptance**
   - `beach-buggy` already parses `fast_path_webrtc`; verify it accepts the default hint and establishes fast-path for a session with no harness. Fix any regressions if hint parsing assumes harness-provided channels.

4) **Control/bridge semantics**
   - Ensure controller forwarder honors fast-path for generic sessions: when fast-path endpoints are present, treat WebRTC channel as primary (or unified fastpath extensions) and keep pairing status at `fast_path`.
   - Preserve harness-specific behavior; generic fast-path should not interfere with existing controller/backoff logic.

5) **Tests**
   - Manager test: register + attach a session with no harness, assert handshake includes `fast_path_webrtc`.
   - Buggy test: supply transport hints with default fast-path; assert connection succeeds (no “reverting to HTTP”).
   - Existing harness tests must still pass.

6) **End-to-end validation**
   - Attach a public host session to a clean private beach, connect via Surfer, confirm fast-path establishment (no auth tag mismatch), actions/state flow.

7) **Smoke script update**
   - Update `scripts/pong-fastpath-smoke.sh` so lhs and rhs players are **public** Beach host sessions, and the mock agent is a **private** beach session. This should mirror manual setup in rewrite-2: create two application tiles (public sessions) and one agent tile (private session), then connect agent tile to the two application tiles—no extra controller config required once generic fast-path works.

## Notes
- Keep passcode auth intact (no weakening of transport auth). The change is to always expose fast-path endpoints with sane defaults for any attached session.
- If time permits, add a feature flag to disable generic fast-path advertising; default should be enabled to meet the requirement.

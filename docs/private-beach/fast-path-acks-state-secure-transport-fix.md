# Fast-Path ACK/State Secure Transport Fix

## Background

The recent fast-path smoke runs revealed a regression limited to the
`mgr-acks` and `mgr-state` data channels. The manager (answerer) creates all
three fast-path channels (`mgr-actions`, `mgr-acks`, `mgr-state`) and now sends
the `__ready__` sentinel on each channel as soon as `dc.on_open` fires. The host
(offerer) polls every channel for that sentinel before it enables the fast-path
transport.

The controller channel continues to handshake successfully, but the ACK and
state channels time out waiting for `__ready__`, eventually closing the SCTP
stream and forcing the agent back to HTTP. This is the root cause of the recent
"duplicate ball" and "HTTP fallback" reports.

## Diagnosis

1. **Offerer Secure Transport Gate**
   - In `apps/beach/src/transport/webrtc/mod.rs` we recently added
     `peer_supports_secure_transport()` to avoid performing the Noise handshake
     when the remote `PeerInfo` does not carry the `mgr-*` label.
   - Only the controller (`mgr-actions`) peer has metadata populated, so the
     gate returns `true` for that channel and `false` for the ACK/State channels.
   - When the gate returns `false`, `OffererSupervisor::connect` logs
     `NOT creating handshake channel (secure transport disabled)` and the host
     remains in plaintext mode for that peer.

2. **Manager Still Encrypts ACK/State Channels**
   - `FastPathSession::set_remote_offer` / `install_ready_sentinel` have not
     changed: both still derive the Noise keys and call `send_text` with sealed
     payloads for the `mgr-acks` and `mgr-state` channels.
   - Manager logs confirm it sends the `__ready__` sentinel repeatedly on every
     fast-path channel (N retries, no failures).

3. **Mismatch Leads to Missing `__ready__`**
   - The host expects plaintext `__ready__` on ack/state, but the manager
     delivers encrypted frames. The host never marks `ready_seen = true`,
     closes the peer connection, and logs `did not receive __ready__ sentinel`.
   - Subsequent fast-path controller promotion fails because both ack/state
     channels are closed, leaving the agent stuck on HTTP fallback.

## Proposed Fix

Ensure the host and manager always agree on whether a fast-path peer uses
secure transport:

1. **Populate Metadata for Fast-Path Peers**
   - When the manager publishes `PeerInfo` for `mgr-acks` and `mgr-state`,
     include `metadata: { "label": "mgr-acks" }` (and `mgr-state` respectively),
     mirroring what we already do for `mgr-actions`.
   - With metadata present, `peer_supports_secure_transport()` will return
     `true`, the host will perform the Noise handshake, and the
     `__ready__` sentinel will be decrypted correctly.

2. **Fallback: Disable the Gate**
   - If we prefer not to rely on metadata, drop or relax the
     `peer_supports_secure_transport()` guard so that all fast-path peers enable
     secure transport. This keeps both sides aligned without waiting on schema
     changes.

## Validation Plan

1. Apply the metadata fix (or remove the gate) and rebuild both the host CLI
   and beach-manager images.
2. Re-run `scripts/pong-fastpath-smoke.sh` end-to-end (let it recreate docker,
   run `--setup-beach`, and collect artifacts).
3. Confirm the host logs show
   ```
   received __ready__ sentinel ... peer_id=<mgr-acks>
   fast-path state channel active
   ```
   and the smoke harness reports success (fast-path ready for LHS/RHS, score
   updates, watchdog count within limits).
4. Repeat with the simpler `scripts/fastpath-smoke.sh` to make sure controller
   behavior remains unchanged.

## Follow-Up

Once the ACK/State channels handshake reliably again:

- Remove the temporary instrumentation that resends `__ready__` every 2 s (or
  leave it behind a trace flag).
- Update `docs/private-beach/pong-fastpath-ack-state-investigation.md` with the
  resolution so future regressions are easier to triage.

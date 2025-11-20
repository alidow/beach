# Debug Prompt: Fast-Path ACK/State Channels

## Background

The Pong fast-path smoke test has regressed: the controller channel (`mgr-actions`) handshakes successfully, but the ACK (`mgr-acks`) and state (`mgr-state`) channels time out during the readiness handshake. The host logs show repeated `beginning to poll for __ready__ sentinel … ready_seen=false … closing peer connection: did not receive __ready__ sentinel`. The manager logs show it repeatedly calling `RTCDataChannel::send_text("__ready__")` for those channels, yet the host never sees the sentinel. The prevailing suspicion is that the host disables secure transport for the ACK/state peers because their `PeerInfo` metadata lacks a `label`, while the manager still encrypts the traffic.

We need a second AI agent to inspect the relevant code paths and confirm (or refute) that hypothesis, and to flag any other mismatches that could explain the failure.

## Prompt for the Debugging Agent

```
You are debugging why the fast-path ACK (`mgr-acks`) and state (`mgr-state`) data channels never complete the readiness handshake in our Pong smoke test. Controller (`mgr-actions`) works; the other two channels log “did not receive __ready__ sentinel” on the host. The manager logs say it sends the sentinel repeatedly.

Please inspect the following areas without bias toward a particular root cause:

1. Host/Offerer code: `apps/beach/src/transport/webrtc/mod.rs`
   - How does `OffererInner::handle_peer_join` decide whether to enable secure transport (`peer_supports_secure_transport`)?
   - Are all incoming data channels published via `pc.on_data_channel`?
   - In the readiness loop (after `beginning to poll for __ready__ sentinel`), under what conditions would we never see `payload.as_text() == "__ready__"`?
2. Manager/Fast-path code: `apps/beach-manager/src/fastpath.rs`
   - Does the manager label each fast-path peer (`mgr-actions`, `mgr-acks`, `mgr-state`) consistently when broadcasting `PeerInfo`?
   - Where does `install_ready_sentinel` send the sentinel? Does it assume encryption is enabled?
3. Metadata flow: `apps/beach-manager/src/state.rs`, `apps/beach/src/server/terminal/host.rs`
   - When the controller peers are registered, do we attach metadata (`label`) for all three channels, or only for `mgr-actions`?
4. Any SCTP/data-channel configuration differences between the controller channel and ACK/State channels that could lead to one being plaintext and the others encrypted.

For each potential issue you find, cite the file/line and explain how it would lead to the host never seeing a plaintext `__ready__` even though the manager believes it sent it. If you find that metadata is indeed missing for `mgr-acks`/`mgr-state`, call it out explicitly; otherwise, flag whatever discrepancy you discover.
```

## Notes

- The debugging agent does not need to run the harness; this is a code-inspection task.
- Avoid biasing the response: the prompt asks for confirmation of the metadata hypothesis **and** other possible culprits.

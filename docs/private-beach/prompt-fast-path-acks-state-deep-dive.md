# Investigation Guide: Fast-Path ACK/State Handshake Failure

## Current Situation

- **Symptom:** The Pong fast-path smoke test succeeds for the controller (`mgr-actions`) channel but the ACK (`mgr-acks`) and state (`mgr-state`) channels never see the `__ready__` sentinel. The host logs `beginning to poll for __ready__ sentinel … ready_seen=false … closing peer connection`, while the manager reports that it sent the sentinel multiple times.
- **Leading hypothesis:** The host’s secure-transport gate disables encryption for peers whose `PeerInfo.metadata["label"]` is missing. Controller metadata is populated, so the host enables Noise and decrypts `__ready__`. ACK/State metadata appears to be absent, so the host stays plaintext while the manager still encrypts, making the sentinel unreadable.
- **Need:** A thorough inspection of the host CLI, manager fast-path stack, and metadata plumbing to confirm (or disprove) the hypothesis and surface any other issues that could explain the failure—without editing code.

## Investigation Checklist

1. **Host Offerer (`apps/beach/src/transport/webrtc/mod.rs`)**
   - Trace `OffererInner::handle_peer_join`, `peer_supports_secure_transport()`, and the readiness loop to understand exactly when the host enables Noise and how it listens for `__ready__`.
   - Confirm that `pc.on_data_channel` publishes every incoming channel and that the readiness loop only considers messages delivered via `WebRtcTransport::try_recv()`.
2. **Manager Fast-Path (`apps/beach-manager/src/fastpath.rs`)**
   - Inspect how `FastPathSession::set_remote_offer` labels each data channel and how `install_ready_sentinel` sends the sentinel (`RTCDataChannel::send_text("__ready__")`).
   - Verify whether the manager assumes encryption is negotiated for every channel and whether `PeerInfo.metadata` is populated consistently.
3. **Metadata Flow / Session Registry (`apps/beach-manager/src/state.rs`, `apps/beach/src/server/terminal/host.rs`)**
   - Follow how the `mgr-*` labels are set when peers are announced and whether the ACK/State peers actually receive metadata.
4. **Transport/Framing Edge Cases**
   - Enumerate all places where the sentinel could be dropped: SCTP stream negotiation, decryption queues, framing (`decode_message`), or controller/fast-path bridging.
5. **Log Correlation**
   - Use artifacts in `temp/pong-fastpath-smoke/<timestamp>/container/*.log` to anchor findings: match peer IDs, handshake IDs, and sentinel attempts.

## Expected Output

Produce a detailed report covering:

1. Handshake flow for each `mgr-*` channel on both host and manager.
2. Secure-transport negotiation rules and the metadata dependency.
3. Every plausible failure point for the sentinel, with supporting file/line references.
4. Final diagnosis (or ranked hypotheses) explaining why ACK/State never see `__ready__`.
5. If uncertainty remains, propose targeted experiments or instrumentations to collect the missing evidence (still without code changes during this investigation).

Use this guide while inspecting the repo; the actual prompt for the next agent is provided separately so it can reference this document directly.

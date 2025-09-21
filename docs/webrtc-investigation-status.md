# WebRTC Signaling Integration Failure – Investigation Notes

## Current Symptom
- The integration test `webrtc_signaling_end_to_end` (in `apps/beach-human/tests/webrtc_transport.rs`) consistently times out waiting for the offer→answer message.
- Offerer → answerer text message (`"ping"`) never arrives even though:
  - ICE negotiation completes (connection state transitions to `Connected` on both peers).
  - Data channels open on both sides (`"offerer data channel open"`, `"answerer data channel open"`).
  - The offerer’s send loop reports successful sends and logs the payload bytes.

## Reproduction
```bash
cargo test -p beach-human webrtc_signaling_end_to_end -- --nocapture
```

## Instrumentation & Changes Made So Far
1. **Transport-level logging** (`apps/beach-human/src/transport/webrtc/mod.rs`):
   - Log transport ID allocation, data channel open/close/error callbacks, message enqueue/dequeue, and send-loop activity (including payload bytes & channel state).
   - Log ICE candidate polling results, including raw candidate strings and `end-of-candidates` markers.
   - Added handshake logging for readiness message attempts (`"__ready__"` from answerer, `"__offer_ready__"` from offerer).

2. **Test adjustments** (`webrtc_signaling_end_to_end`):
   - Loops over answer-side `recv` to skip the diagnostic `"__offer_ready__"` handshake payload when present.
   - Kept default 5s timeouts (30s trials didn’t help).

3. **Handshaking experiments**:
   - Answerer sends `"__ready__"` when its data channel opens; offerer waits for this before continuing.
   - Offerer now retries by sending `"__offer_ready__"` after the wait succeeds (though the test ignores the extra message).

4. **Candidate exchange verification**:
   - Added polling of `/offer|answer/candidates` endpoints and ensured host candidates flow both ways via the temporary signaling server.
   - Confirmed `poll_remote_candidates` waits until the peer has a remote description before consuming batches.

## Key Observations
- **Offerer receives traffic; answerer does not**:
  - After the handshake, the offerer logs `received frame len=22 seq=0` (meaning it receives answerer’s `__ready__` or `pong`).
  - The answerer never logs a `received frame` for the offerer’s application payload (beyond possibly the handshake), even though the send loop reports success.
- **Message queueing works locally**: The in-memory WebRTC pair (unit test `webrtc_bidirectional_transport_delivers_messages`) still passes, proving the serialization layer is intact.
- **No data channel errors**: The new `on_error`/`on_close` callbacks never fire; ready state logged as `Open` during sends.
- **Transport IDs**: Offerer allocates `TransportId(1)`; answerer takes `TransportId(3)`; IDs remain stable across runs (handshake mismatch is unlikely to be ID-related).
- **Handshake not reliable**: `⚠️ offerer did not receive readiness ack` still appears intermittently, meaning the answerer’s `__ready__` message doesn’t always reach the offerer despite logging that `answerer transport ready` (transport constructed in `on_data_channel` handler).

## Hypotheses (Unresolved)
1. **Answer-side handler race**: The `on_data_channel` callback creates the transport and immediately sends `__ready__`. Possibly the message is sent before the offerer’s data channel is ready to receive; the send completes locally but never leaves the buffer.
2. **Signaling server candidate timing**: Although we now log candidates, we have only host candidates; maybe srflx/reflexive or relay candidates are required in this environment? (But connection reaches `Connected`, so at least one candidate pair succeeds.)
3. **Tokio thread context / runtime crossing**: The transport stores an `std::sync::mpsc` receiver; the data channel callback runs on async context. There might be blocking behaviour when both peers simultaneously wait on `std::sync::mpsc::Receiver::recv_timeout`.
4. **Sequence mismatch**: Offerer’s outbound sequence is `0` (`ping`) while answerer handshake `__ready__` also uses `sequence=0`. If the answerer handshake reuses sequence `0`, the offerer might treat the subsequent `ping` as duplicate and drop it (needs verification—see `decode_message` path for duplicates?).
5. **Transport handshake order**: Offerer waits for `__ready__` before sending `__offer_ready__`, but if the answerer consumes the handshake internally before exposing the transport, the real message could be lost.

## Next Tests / Work Items
- Inspect `TransportMessage::sequence` handling to ensure duplicates aren’t dropped.
- Trace `offer_transport`’s `recv` logs around handshake to confirm what payload is received (if any) prior to `ping`.
- Add detailed logging in `decode_message` / `encode_message` to confirm message framing lengths match expectations.
- Compare the integration test handshake with the in-memory pair to see what differs in order / timing.
- Consider replacing `std::sync::mpsc` with an async channel to rule out blocking interaction between async callbacks and synchronous queue.

## Reproduction Artifacts
- Failing test command: `cargo test -p beach-human webrtc_signaling_end_to_end -- --nocapture`
- Relevant modules: `apps/beach-human/src/transport/webrtc/mod.rs`, `apps/beach-human/tests/webrtc_transport.rs`
- Logs to inspect: `/tmp/webrtc.log` (created by previous runs) include examples of candidate strings, send/recv logs, and timeout messages.

This document should provide enough context for another engineer to continue the investigation from the current logging-enhanced state.

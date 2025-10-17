# WebRTC Secure Handshake Investigation (Oct 16–17, 2025)

## Symptom

- Client launches `cargo run -p beach -- ssh …` against EC2 offerer (Singapore) and consistently stalls at **“Connected – syncing remote session…”**.
- WebRTC negotiation falls back to WebSocket with error `transport setup failed: secure handshake timed out`.
- Host log mirrors the timeout (`offerer data channel did not open` or handshake timeout), so the secure data channel never transitions to an encrypted transport.

## What We Verified / Fixed

1. **Binary mismatch eliminated**
   - Added `--verify-binary-hash` option to copy flow; confirmed client & host are running identical timestamped build (`0.1.0-20251016230021`).
   - Enabled SCP compression and atomic rename (`beach.upload → beach`).

2. **ICE candidate encryption**
   - Earlier runs showed fallback to passphrase; instrumentation confirmed mismatch in associated data.
   - Adjusted send/receive AAD to use assigned peer ids; both sides now decrypt ICE candidates with the handshake key.

3. **Secure handshake telemetry**
   - Added trace logging in `secure_handshake.rs` for key hashing, Noise message sizes, verification frames, and total duration.
   - Client log (responder) now shows `noise handshake waiting for inbound message` and never receives message 0.
   - Host log (initiator) shows it **does** write the first Noise frame but waits on `await_read` immediately after.

4. **Manual host launch**
   - To eliminate `nohup` buffering, we now run: `BEACH_LOG_LEVEL=trace BEACH_LOG_FILE=/tmp/beach-host.log ./beach host …` and join from another terminal to capture both sides at TRACE.
5. **Answerer pre-registers handshake inbox**
   - When the incoming channel label matches `beach-secure-handshake`, we now create the `HandshakeInbox` and attach `on_message` *before* the async `on_data_channel` handler yields.
   - The responder reuses that inbox inside `run_handshake`, so any frames delivered between channel creation and task spawn are buffered instead of dropped.

## Leading Theory

The initiator writes the first Noise packet the moment the data channel fires `OnOpen`. The responder registers its `on_message` handler only after spawning the handshake task, so the very first packet is likely delivered **before** the callback is attached. Because the channel is ordered and reliable, any packet dropped at that stage isn’t retransmitted; the responder blocks forever on `recv().await` and our 10 s watchdog fires.

Evidence:

- Offerer log: `noise handshake wrote initial message … bytes=32` followed immediately by `noise handshake waiting for inbound message`.
- Responder log: `handshake channel is now open…` followed immediately by `noise handshake waiting for inbound message` with no preceding `handshake_channel_inbound_raw` (the new trace never fires).
- With the answerer now pre-registering the inbox, the next manual run should tell us whether this hypothesis holds or if the loss happens deeper in the stack.

## Next Steps

1. **Validate pre-registered handshake inbox** (pending):
   - Rebuild/upload the binaries with the latest `HandshakeInbox` changes on both host and client.
   - Re-run the manual EC2 session and confirm the responder emits `handshake_channel_inbound_raw` followed by Noise `action="read" message_index=0`.
   - If the first packet is still missing, capture fresh TRACE logs on both sides to revisit the hypothesis.

2. **Double-check channel ordering**
   - Confirm handshake channel is `ordered=true` (current `handshake_channel_init` sets it) and `max_retransmits` is unset.
   - If unordered, first packet could be dropped even after buffering.

3. **Adjust timeout**
   - Once messages flow, revisit the hard-coded `timeout(Duration::from_secs(10), secure_rx)`; may need to increase or surface as configuration.

4. **Regression safeguard**
   - Add integration test or harness that spins up offerer/answerer pair, asserts successful Noise handshake, and fails if responder never sees message 0.

## Artifacts

- Latest client log: `/Users/arellidow/beach-debug/client.log`
- Latest host log: `/Users/arellidow/beach-debug/host.log`
- Session/handshake example: `session_id=2ddd852b-f024-44c6-85b9-390bf8351ded`, `handshake_id=21327969-9248-4179-addb-d9d42f1e10da`.

This document captures the current state so another engineer (or future me) can resume by finishing the handshake inbox wiring, rerunning the manual test, and inspecting the logs for the expected Noise receive traces.

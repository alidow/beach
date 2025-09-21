# WebRTC Signaling Integration – Current Status (2025‑09‑21)

## Symptom Recap
- `apps/beach-human/tests/webrtc_transport.rs::webrtc_signaling_end_to_end` still times out waiting for the offerer’s `"ping"` to reach the answerer.
- ICE negotiation succeeds and both peers report their `RTCDataChannel` in `Open` state.
- The offerer logs a successful send of the ping frame (`bytes_written=28`, `ready_state=Open`) yet the answerer never surfaces the payload; only the handshake frame (`__offer_ready__`) appears before the receive loop times out.

## Reproduce & Capture Logs
```bash
# From repo root
RUST_LOG=beach_human::transport::webrtc=trace \
  cargo test -p beach-human --test webrtc_transport \
  webrtc_signaling_end_to_end -- --nocapture
```
- Look for the `sent frame` / `queueing outbound message` lines in the test output.
- We currently emit extra instrumentation to `/tmp/webrtc*.log` only if the `BEACH_DEBUG_LOG` env var is set when running binaries; the test path logs directly to stderr/stdout via `tracing`.

## Investigation Timeline (latest first)
1. **Async/runtime boundary fixes**
   - Replaced the old `std::sync::mpsc` inbound queue in `apps/beach-human/src/transport/webrtc/mod.rs` with `crossbeam_channel` to avoid blocking the async runtime when receiving frames.
   - Added `spawn_on_global` helper so the outbound sender loop always runs on the static multi-thread tokio runtime, avoiding starvation when the caller uses `tokio::runtime::Builder::new_current_thread` (the default for tests).
   - Wrapped the offerer’s readiness wait (`transport.recv(CONNECT_TIMEOUT)`) in `spawn_blocking` so the test runtime no longer parks while the handshake message is pulled off the queue.

2. **Test harness adjustments**
   - Added `recv_via_blocking` helper in `apps/beach-human/tests/webrtc_transport.rs` so the answer-side waits for frames using `spawn_blocking` instead of blocking the async executor thread.
   - Skipped the `"__offer_ready__"` handshake blob before asserting on `"ping"`.

3. **Logging / instrumentation**
   - Transport now logs dequeues, payload lengths, SCTP buffered amount before/after each send, and all handshake warnings at `trace` or `warn` level.
   - The test prints when it issues the ping to correlate with the transport logs.

## What We Observed After These Changes
- The offerer successfully sends two frames: the answerer’s readiness ACK (sequence 0) and the ping (sequence 1). The second send shows `buffered_before` ≈ 20–50 bytes and `buffered_after` slightly higher, so the SCTP buffer is not obviously saturated.
- The answerer’s receive logs never show `sequence=1`; only the handshake frame arrives. Immediately afterwards the test hits its 5 s timeout and panics.
- Handshake warnings remain (`offerer did not receive readiness ack`) roughly 10 s after start, even though the answerer does issue `__ready__`. We now drain that message via `spawn_blocking`, so the warning indicates the answerer’s ACK is occasionally lost before it reaches the transport queue.
- No data-channel errors or closures fire; both sides stay `Open` until the test aborts.

## Likely Root Cause Candidates
1. **Backpressure / ordering on the SCTP data channel** – even though the buffered amount remains modest, we never wait for `buffered_amount_low` events and always fire a second frame immediately. The reference implementation (`apps/beach`) chunks and throttles sends (sleeping 100 µs between chunks) which may prevent the handshake payload from being reordered/dropped.
2. **Blocking receivers elsewhere** – we only wrapped one `Transport::recv` in `spawn_blocking`. Other call sites (client host loops, telemetry helpers) still call `recv()` directly; if any executes on the current-thread runtime during the test, it can still starve the sender task we moved onto the global runtime. We have not audited those paths yet.
3. **Handshake sequencing** – the beach-human transport still relies on special `__ready__` / `__offer_ready__` frames over the data channel. In `apps/beach`, signalling is driven through the `RemoteSignalingChannel` and control data channel opening events; they don’t reuse the data channel for readiness messages. The extra handshake frame might race with the first application payload on some runs.

## Reference Implementation Clues (`apps/beach`)
Reading `apps/beach/src/transport/webrtc/mod.rs` (known-good path) highlights a few structural differences:
- **Async channels everywhere**: inbound/outbound queues use `tokio::mpsc` guarded by async locks, so no synchronous `recv_timeout` blocks a runtime thread.
- **Chunking & throttling**: large messages are split into ≤60 KB chunks and a short `sleep` is inserted between chunks. Even for small messages, sends happen inside a `tokio::spawn` task dedicated to the channel.
- **Dedicated channel purposes**: control and output data share separate channels with their own open/close tracking, which may reduce contention and ordering issues for early control frames.
- **Signaling-driven readiness**: connection readiness is tracked via the signaling channel and control channel state changes, not ad-hoc payloads on the data channel.

These differences suggest two immediate hypotheses for beach-human:
1. **Lack of send throttling or buffered-amount handling** is letting the SCTP implementation reorder/drop the second frame under some conditions. Implementing chunking/backpressure similar to `apps/beach` (set `buffered_amount_low_threshold`, wait on `on_buffered_amount_low`) could unblock delivery.
2. **Control-plane handshake should live on signaling / explicit control channel**. Eliminating the `__ready__`/`__offer_ready__` payloads in favour of the signaling layer (as beach already does) would remove the race where the application message follows immediately after an internal handshake frame.

## Trace Logging & Panic Capture Plan
- Introduce lightweight wrappers (e.g. `await_trace!(label, future)`) around the handful of hot async points — WebRTC send loop, candidate polling, handshake waits — so we log before entering and after resuming each `.await`. These wrappers can live in `apps/beach-human/src/transport/webrtc/mod.rs` and be gated behind `RUST_LOG` `trace` level so normal runs stay quiet.
- Provide traced variants of `tokio::spawn`, `tokio::spawn_blocking`, and the global runtime spawn helper so every task/worker thread logs start/finish and the number of frames processed. This will show us if any worker silently stops.
- Install a global panic hook (`std::panic::set_hook`) that records thread name, task description, and a backtrace to the tracing logger. Additionally, after every `tokio::spawn` we should `let handle = tokio::spawn(...);` and `handle.await` in supervising tasks to surface panics as logged errors.
- For OS threads (e.g. in tests) wrap `spawn_blocking` jobs in a helper that catches panics (`std::panic::catch_unwind`) and maps them into error logs so we can categorically rule out unexpected unwinds killing the sender/receiver loops.

## Next Concrete Steps
1. **Add the trace wrappers & panic hook** so we capture every await/spawn boundary and all panics while the integration test runs.
2. **Audit remaining `Transport::recv` usages** – wrap them in `spawn_blocking` (or migrate to async channels) so no synchronous call stalls the runtime while the WebRTC sender is live. Relevant files: `apps/beach-human/src/main.rs:705`, `apps/beach-human/src/transport/mod.rs:282`, `apps/beach-human/src/client/terminal.rs:96`, etc.
3. **Add buffered-amount backpressure** – set `data_channel.set_buffered_amount_low_threshold` and wait for `on_buffered_amount_low` notifications before draining the unbounded queue. Record the thresholds in the logs to confirm the buffer drains before we enqueue the ping.
4. **Move readiness handshake off the data channel** – mirror `apps/beach` by tracking readiness through signaling (or a dedicated control channel). Once the transport only carries real application frames, re-run the test to confirm the ping arrives.
5. **Optional**: port over the chunking helper from `apps/beach` so large frames (snapshots) don’t flood the queue during future integration tests.

## Quick Start for the Next Engineer
1. Pull latest main (commit `147f3ed` is the current checkpoint).
2. Run `cargo test -p beach-human --test webrtc_transport webrtc_signaling_end_to_end -- --nocapture` to see the timeout and current logs.
3. Compare `apps/beach-human/src/transport/webrtc/mod.rs` against `apps/beach/src/transport/webrtc/mod.rs` to port the missing backpressure + control-channel logic.
4. Once the test passes locally, update this document with the new behaviour and remove any temporary logging.

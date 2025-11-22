# Fast-Path WebRTC Chunking Plan (Beach Transport Layer)

Goal: never hit SCTP data channel size limits again, even with large terminal/state frames. Implement mandatory chunking/reassembly inside `apps/beach/src/transport/webrtc` so all payloads (controller state, terminal frames, telemetry) are safely fragmented and reassembled transparently. Greenfield: ship always-on; no staged rollout.

## Design
- **Placement**: wrap send/receive in the shared WebRTC transport (`WebRtcTransport::send` and the on-message handler) so all higher layers are unchanged.
- **Envelope (chunk frame)**:
  - `msg_id`: u128/UUID string (unique per logical message)
  - `seq`: u32 chunk index (0-based)
  - `total`: u32 total chunk count
  - `payload`: raw bytes (if binary) or base64 (if JSON frame chosen)
  - `kind` (optional): passthrough/debug of original message type
  - A single-frame message is encoded with `total=1`, `seq=0`.
- **Encoding**: prefer binary framing (small header + payload) to avoid base64 inflation; fallback JSON is acceptable if implementation time is tight. Include a 1-byte version tag at start for future-proofing.
- **Chunk sizing**:
  - `MAX_CHUNK_BYTES = 16 * 1024` (safe under SCTP/UDP + headers).
  - `MAX_MESSAGE_BYTES = 1 * 1024 * 1024` (hard cap; reject/log if exceeded).
- **GC / safety**:
  - Per-connection reassembly map keyed by `msg_id`.
  - GC timeout for partial messages: 10s (drop and log if incomplete).
  - Validate `total` and `seq` bounds; drop malformed frames.
- **Ordering**:
  - Data channels remain ordered/reliable; still tolerate duplicates/out-of-order in the reassembler.
- **Flow control**:
  - Respect existing send backpressure; if `buffered_amount` exceeds a threshold, defer chunk emission (or early-return with retry/backoff).
- **Feature flag**:
  - None for normal use; chunking is always on. Allow a hidden debug override (`BEACH_WEBRTC_CHUNKING=0`) only for emergency debugging.

## Implementation steps
1) **Schema + helpers**
   - Add a `chunk` module (e.g., `apps/beach/src/transport/webrtc/chunk.rs`) with:
     - Structs for envelope (binary or JSON) and errors.
     - `fn split(payload: &[u8]) -> Vec<ChunkFrame>` and `fn encode(frame) -> Bytes`.
     - Reassembler struct with `ingest(frame) -> Option<Vec<u8>>` plus GC of stale entries.
     - Constants for limits and GC timeouts.
2) **Wire into send path**
   - In `WebRtcTransport::send` (or equivalent), before writing to `data_channel`:
     - If chunking disabled → existing behavior.
     - Else: size-check (`MAX_MESSAGE_BYTES`), split, encode, emit each chunk; respect bufferedAmount/backpressure.
   - Add log on first chunked message per connection for observability.
3) **Wire into receive path**
   - In the data channel `on_message` handler:
     - Detect chunked frame by version tag/header.
     - Pass to reassembler; when complete, forward the reassembled payload to existing consumer.
     - If not chunked, pass through unchanged for backward compatibility (during rollout).
4) **Error handling**
   - If message too large → log warn and drop (avoid crashing transport).
   - If GC drops partial message → log debug/warn with `msg_id` and counts.
   - If malformed envelope → log and drop.
5) **Metrics**
   - Counters: chunked_messages, chunks_emitted, partial_gced, malformed_frames, oversized_dropped.
   - Optional histograms: reassembly_latency_ms.
6) **Config surface**
- Env vars:
  - `BEACH_WEBRTC_CHUNKING` (optional debug override; defaults to enabled)
  - `BEACH_WEBRTC_MAX_CHUNK_BYTES` (optional override)
  - `BEACH_WEBRTC_MAX_MESSAGE_BYTES` (optional override)
7) **Tests**
   - Unit (pure Rust):
     - Split/reassemble round-trip.
     - Missing chunk → None + GC cleanup.
     - Duplicate/out-of-order delivery still reassembles.
     - Oversized message rejected.
   - Integration (Rust):
     - Simulate data channel mock that records sent frames; ensure large payload (>128 KB) is chunked and reassembled.
   - Manual:
     - Run Pong showcase with large viewport (80x80), confirm no SCTP “outbound packet larger…” and no fast-path disconnects; verify agent moves paddles.
8) **Rollout**
   - Build and ship manager + hosts together (chunking always on).
   - Re-seed stack, run Playwright fast-path test + Pong manual exercise.
   - Keep logs at warn for malformed/oversize to detect regressions.

## Risks / mitigations
- **Mixed-version peers**: allow passthrough of unchunked frames and only wrap when the peer understands chunking; upgrade manager + hosts together.
- **Memory growth from partials**: enforce GC timeout and cap total in-flight partials with eviction policy (e.g., drop oldest when map exceeds N entries).
- **Perf overhead**: use binary framing and avoid extra allocations (reuse buffers where possible).

## Files to touch (expected)
- `apps/beach/src/transport/webrtc/mod.rs` (send/receive integration)
- New: `apps/beach/src/transport/webrtc/chunk.rs` (or similar)
- Tests under `apps/beach/src/transport/webrtc/` or `apps/beach/tests/`

## Acceptance criteria
- Large terminal frames (>100 KB) transmit without SCTP size errors or fast-path disconnects.
- Agent commands no longer time out under high viewport sizes.
- Chunking is on by default, configurable via env, and covered by unit/integration tests.

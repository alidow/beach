# beach-human Performance Migration Status

_Last updated: 2025-09-22_

## Current State

- **Binary wire format (Phase 7a) is in place**. `apps/beach-human/src/protocol/wire.rs` defines the packed frame layout with varints, packed cells, and style-table references. All encode/decode helpers are committed and tested.
- **Server pipeline (Phase 7b) now emits binary frames end-to-end**. `TransmitterCache` in `apps/beach-human/src/main.rs` deduplicates row/cell/style updates per sink, and the host always uses `send_host_frame(...)` with the packed protocol.
- **Client runtime (Phase 7c) consumes binary frames end-to-end**. `TerminalClient` decodes `WireHostFrame`, applies the new row-segment updates, and emits binary input frames back to the host.
- **Tests & perf harness (Phase 7d)**: CLI/unit tests (`webrtc_mock_session_flow`, `transport_sync`, `session_roundtrip`) now exchange `WireHostFrame`/`WireClientFrame` directly; the legacy JSON helpers in `main.rs` are gone. `tests/perf_harness.rs` still measures JSON vs. binary sizes (~80% reduction) for historical comparison.

## Outstanding Work

- Validate that WebRTC signaling remains the only JSON surface area and add binary assertions around the data channel path.
- Capture encode/decode timings separately for snapshots vs. deltas so we can target the next round of micro-optimisations with real telemetry.

## Next Steps

1. **Tighten WebRTC harness**
   - Update `tests/webrtc_transport.rs` to assert that all data-channel payloads are binary while leaving signaling JSON intact.
   - Add coverage for retransmit/heartbeat lanes so sentinel filtering (`__ready__`) cannot regress.

2. **Confirm transport coverage**
   - Exercise IPC and WebRTC transports with the binary protocol enabled end-to-end and compare frame drops/latency vs. ssh+tmux.
   - Ensure the mock signaling path emits binary frames for retries/heartbeats.

3. **Performance tuning ideas (post-cleanup)**
   - **Run-length encoding** for long whitespace spans or repeated glyphs inside `RowSegment` updates.
   - **Dictionary compression for style ids** beyond the current 32-bit fields (e.g., Huffman over style deltas or 12-bit packed indices).
   - **Optional zstd framing** for history/backfill lanes when latency is less critical than bandwidth.
   - **Adaptive delta batching**: adjust flush cadence based on observed RTT / pending queue depth instead of fixed iteration loop.
   - **Predictive echo heuristics**: piggyback ack timing to drop stale predictions sooner, reducing client redraw work.
   - **Telemetry hooks**: extend `PerfGuard` counters to track encode/decode time separately for row segments vs. row snapshots.

4. **Benchmark rerun**
   - Re-run the perf harness (`cargo test -p beach-human --test perf_harness -- --ignored --show-output`) and the interactive vim benchmark to quantify headroom vs. SSH+tmux after each optimisation.
   - Capture logs with `BEACH_HUMAN_PROFILE=1` to compare `sync_send_bytes`, `client_handle_frame`, and `pty_chunk_process` before vs. after micro-optimizations.

## Handoff Notes

- No environment flags are required anymore; the binary protocol is always on. JSON remains only in the WebRTC signaling envelope shared with beach-road.
- Continue from this state by hardening the WebRTC harness, then tackle the optimisation ideas above.

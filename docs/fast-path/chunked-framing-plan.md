# Fast-Path Chunked Framing Rollout

## High-Level Goals

- **Unify framing:** Route every fast-path payload (actions, acks, state diffs, health, future telemetry) through a single chunked frame format so no write exceeds SCTP’s ~64 KiB ceiling.
- **Transparency for callers:** Controllers and hosts still hand opaque JSON/binary blobs to the transport helpers; the chunker handles segmentation/reassembly behind the scenes.
- **Bidirectional safety:** Both the host (`apps/beach`) and manager (`apps/beach-manager`) must chunk outbound messages and reassemble inbound frames before JSON decoding, eliminating asymmetric failures.
- **Feature-flagged rollout:** Introduce a capability bit in the fast-path hints so chunk-aware binaries interoperate with legacy peers until the entire fleet is upgraded.
- **Observability:** Emit logs/metrics that show when chunking is active, frame counts per message, and any reassembly failures so regressions are obvious.

## Regression Tests (Should Fail Before the Work Lands)

1. **Large state diff over fast-path**
   - _Setup:_ Patch `StatePublisher` to emit a synthetic 80×200 grid snapshot (≈45 KB JSON) and force `FAST_PATH_MAX_PAYLOAD_BYTES` to 16 KB so the code always attempts fast-path first.
   - _Expectation:_ With today’s raw `send_text`, the SCTP stack stalls, and `fast_path.state` logs `outbound packet larger than maximum message size`. After chunking, the same test should deliver the diff and persist it via `record_state`.
   - _How to automate:_ Add an integration test under `apps/beach/src/server/terminal/host.rs` that feeds a mocked `FastPathStateChannel` and asserts `transport.send_text` is called multiple times with ≤ chunk size slices.

2. **Manager receives chunked action stream**
   - _Setup:_ Unit test in `apps/beach-manager/src/fastpath.rs` that fabricates two `DataChannelMessage` chunks totaling >20 KB and pushes them through the new reassembler before `parse_state_diff`.
   - _Expectation:_ Before implementation the parser sees the raw chunk header and fails JSON parsing; after implementation the reassembler hands clean JSON to `parse_state_diff`.

3. **Controller action chunk loopback**
   - _Setup:_ Extend `crates/beach-buggy/src/fast_path.rs` tests so `FastPathConnection::send_acks` emits a payload deliberately larger than 16 KB. Feed the resulting chunk series back through a decoder helper and assert the reconstructed envelope matches the original `ActionAck`.
   - _Expectation:_ Current code panics on serialization or the data channel rejects the send; after chunking the round trip succeeds.

4. **Backward compatibility switch**
   - _Setup:_ API-level test in `crates/beach-buggy` that instantiates two fake peers, one advertising `chunked_v1` and one not. When only one side supports chunking, the code should fall back to the legacy single-frame path.
   - _Expectation:_ Before the feature flag exists, negotiation always fails; after rollout, mixed clusters pass this test.

Document these tests in CI (or at least as manual scripts) so we can confirm they flipped from “fail” to “pass” once the chunker is wired in.

## Detailed Implementation Plan

1. **Inventory fast-path payloads**
   - Trace every `send_text`/`on_message` in `crates/beach-buggy/src/fast_path.rs`, `apps/beach/src/server/terminal/host.rs` (`StatePublisher`, health reporters), and `apps/beach-manager/src/fastpath.rs` (action fan-out, ack/state listeners).
   - Capture typical payload sizes from logs to pick a safe chunk budget (≈14 KB after headers) and document the SCTP rationale inline.

2. **Shared framing primitives (beach-buggy)**
   - Add a `FastPathFrame` module with:
     - `FAST_PATH_FRAME_VERSION`, `FAST_PATH_CHUNK_SIZE`, and `enum FastPathKind { Action, Ack, State, Health, Custom(String) }`.
     - `struct FastPathChunkHeader { version, kind, message_id, chunk_idx, chunk_count, payload_len }`.
     - `fn chunk_payload(kind, &[u8]) -> impl Iterator<Item = Vec<u8>>` that prepends the header and yields ≤ chunk-size slices (supporting both text and binary DataChannel modes).
     - `FastPathReassembler` (LRU capped) that buffers chunks keyed by `(kind, message_id)` and returns the original `Vec<u8>` once all parts arrive or times out stale entries.
   - Extend `FastPathConnection` with helpers (`send_frame`, `send_bytes`, `subscribe(kind)`) that hide the chunk math from callers.

3. **Host updates (`apps/beach`)**
   - Replace `StatePublisher::try_fast_path`’s manual `FAST_PATH_MAX_PAYLOAD_BYTES` guard with the new `send_frame(FastPathKind::State, diff_json.into_bytes())`.
   - Ensure idle snapshot and health reporters reuse the same helper.
   - On the receive side (`wire_action_handler`), run every `DataChannelMessage` through `FastPathReassembler` before calling `parse_action_message`, so the host can accept chunked manager frames as soon as it upgrades.

4. **Manager updates (`apps/beach-manager`)**
   - Wrap the outbound action loop in `send_actions_over_fast_path` with the chunked writer.
   - For ack/state listeners, replace `parse_*` callers with a reassembly step. When the decoder yields a full payload, run the existing JSON parsing logic.
   - Emit metrics for reassembly failures (missing chunk, duplicate id, timeout) tagged with the channel label (`mgr-actions`, `mgr-acks`, `mgr-state`).

5. **Capability negotiation**
   - Extend the fast-path hint payload (e.g., `transport_hints.fast_path_webrtc.features = ["chunked_v1"]`).
   - Hosts advertise support via a new query param or ICE hint; managers only send chunked frames when both peers set the flag. Keep legacy single-frame behavior as a temporary fallback.

6. **Testing & validation**
   - Add unit tests for encoder/decoder round trips and corruption cases in `crates/beach-buggy`.
   - Add manager-side tests that confirm `record_state`/`ack_actions` receive reconstructed payloads.
   - Update `scripts/fastpath-smoke.sh` (or add a variant) to inflate terminal payloads and assert fast-path stays connected.

7. **Rollout**
   - Deploy chunk-aware hosts first (they can decode chunked frames but still transmit legacy frames until the flag flips).
   - Once a majority of hosts advertise `chunked_v1`, enable chunked sends on the manager via a config toggle.
   - Remove the legacy one-shot path once metrics show 100 % chunk-capable peers. Document the migration timeline in `docs/private-beach/fast-path-unification-plan.md`.

8. **Documentation & follow-ups**
   - Update `docs/helpful-commands/pong.txt` and `docs/private-beach/pong-controller-queue-incident.md` with a troubleshooting blurb referencing chunked framing.
   - Consider switching to binary DataChannel messages later; the new framing already supports raw bytes, so the follow-up becomes trivial.

This plan ensures every fast-path message respects SCTP limits, keeps callers unaware of the underlying complexity, and bakes in the tests and rollout steps we need to deploy safely.

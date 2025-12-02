# Unified Transport Pub/Sub Refactor Plan

Goal: make the unified WebRTC channel a simple pub/sub message bus. `apps/beach` only handles transport/bus plumbing; `beach-buggy` and `beach-manager` are just publishers/subscribers on topics (e.g., `controller/input`, `controller/ack`, `state/host`, `state/health`). No auto-attach hints, no host knowledge of managers or buggy.

## Architecture
- Introduce a transport-agnostic bus trait (in `crates/beach-buggy` or a new `transport-bus` module):
  - `subscribe(topic: &str) -> stream<Frame>`
  - `publish(topic: &str, bytes: &[u8])`
  - Topics are strings; frames are opaque bytes.
- Unified WebRTC adapter implements the bus on top of the single datachannel. It only multiplexes by topic/kind.
- `apps/beach`:
  - Establishes unified WebRTC transport and instantiates the bus adapter.
  - Exposes the bus handle to consumers; does **not** know about controllers/managers/buggy/private beaches.
  - Deletes auto-attach hint logic; the bus is always “on” once the transport is up.
- `beach-buggy`:
  - Subscribes to `controller/input`, writes to PTY; publishes `controller/ack`.
  - Subscribes/publishes any other controller-related topics it needs.
- `beach-manager`:
  - Uses the bus to send controller actions (`controller/input`) and receive acks (`controller/ack`).
  - Uses the bus for any other host-facing topics it needs (e.g., devtools, telemetry).

## Incremental steps
1) Add the bus trait + unified WebRTC bus adapter (wrap existing unified transport framing into `subscribe/publish`). Keep existing behavior; do not remove anything yet.
2) Switch controller handling in `beach-buggy` to consume/publish via the bus adapter; host just hands off the bus handle.
3) Switch manager controller paths to use the bus (subscribe for acks, publish inputs).
4) Remove auto-attach hint parsing/logic and any controller-specific plumbing inside `apps/beach`; the bus attaches unconditionally when unified transport is ready.
5) Clean up labels/metadata so they’re only used for auth/ids; no behavior gates.

## Tests to add
- Bus adapter roundtrip over IPC transport: publish on topic, receive on subscribe.
- Controller input→ack over bus with no hints: inject `controller/input` frame, assert PTY write and `controller/ack` emitted.
- Manager-side: publish `controller/input`, ensure ack observed over bus.
- Integration: minimal end-to-end with unified transport and bus adapter (can reuse IPC transport).

## Notes/constraints
- Keep message payloads opaque to the bus; don’t add manager/buggy concepts to `apps/beach`.
- Stay unified-only; no HTTP fallback for controller paths.
- Preserve existing logging targets where helpful, but the “auto-attach hint” and “controller attach” gates should go away.

## Progress 2025-02-12
- Transport bus lives in `crates/transport-bus` with a unified WebRTC adapter (`apps/beach/src/transport/bus.rs`) that routes topics over the single datachannel; tests cover IPC roundtrip plus controller input/ack on the bus.
- `beach-buggy` now owns controller bus adapters (`publisher.rs`, `subscriber.rs`) that subscribe to `controller/input`, write to the PTY handler, and publish `controller/ack`.
- `apps/beach` brings the unified bus up for every WebRTC transport and removed controller auto-attach gating/hints; controller actions flow only through the bus subscriber/publisher.
- `apps/beach-manager` publishes controller inputs and listens for acks via new bus helpers; controller forwarder no longer depends on auto-attach metadata.
- New tests run: `cargo test -p beach-buggy subscriber`, `cargo test -p beach-manager decode_ack_envelope`, `cargo test -p beach controller_input_ack_round_trip_ipc`.

## Prompt to hand to another Codex instance
```
You are in /Users/arellidow/development/beach. Implement the unified transport pub/sub refactor per docs/transport-unified-bus-refactor.md:
1) Add a transport-agnostic bus trait (subscribe/publish topics) and a unified WebRTC adapter that multiplexes topics over the single datachannel.
2) In apps/beach, always stand up the unified bus when the WebRTC transport is ready; remove controller auto-attach hint gating. apps/beach should not depend on manager/buggy/private-beach concepts—only transport + bus.
3) Move controller handling into beach-buggy: consume `controller/input` from the bus, write to PTY, publish `controller/ack` on the bus.
4) Update manager to publish controller inputs and listen for acks over the bus.
5) Remove the legacy auto-attach hint plumbing and any controller-specific attachment logic in apps/beach; keep labels only for auth/ids.
6) Add tests: bus IPC roundtrip; controller input→ack over bus with no hints; manager publish/ack over bus; a minimal unified transport + bus integration (can use IPC).
Run targeted tests you add; don’t revert other changes. Keep everything unified-only (no HTTP fallback for controller).
```

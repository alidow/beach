# Multi-Peer WebRTC Signaling & Input Ordering Plan

_Last updated: 2025-09-29_

## Goals
- Support up to 100 concurrent WebRTC data channels between a single host (`beach-human`) and many viewers (`beach-web`, CLI joiners, future native clients).
- Eliminate the single-offer bottleneck by multiplexing all signaling over the existing WebSocket (`/ws/:session_id`) and avoiding the HTTP poll endpoints.
- Prevent stale transports from injecting input once a new handshake supersedes them, while preserving the low-latency properties we rely on today.
- Introduce a lightweight, Mosh-style ordering model so the host can discard client keystrokes that were produced against an out-of-date terminal view.
- Keep the implementation lean (no per-handshake Redis writes, predictable hot-path data structures) so we stay within the perf budget.

## Current Status (2025-09-29)
- The earlier regression fix restored `remote_generation`, but the host signaling client still tracks only **one** active remote peer. Any ICE candidate or ping from another connected client steals that slot mid-handshake.
- When the browser joins after the Rust CLI, the host posts an offer, the browser answers, and the host immediately sees another CLI ping, decides the remote changed, posts a fresh offer, and discards the browser’s answer. Beach-web keeps polling `/webrtc/answer` and never sees a data channel.
- We need the host to “lock” onto a peer while a handshake is in flight so that other clients cannot pre-empt the negotiation. This is a stepping stone toward the full handshake-ID multiplexing described below.

## High-Level Architecture
1. **Per-handshake identifiers.** Every transport negotiation is scoped by a freshly generated `handshake_id` (UUID). Messages routed via beach-road must include this id so the host can manage multiple peer connections at once.
2. **WebSocket-only signaling.** SDP offers/answers and ICE candidates flow exclusively over WebSocket frames. The REST `POST /webrtc/offer|answer` endpoints become legacy fallbacks (still registered for now but marked deprecated).
3. **Explicit handshake lifecycle.**
   - Host emits `transport_propose` with `{handshake_id, kind: 'webrtc', constraints}`.
   - Client acknowledges via `transport_accept` and starts exchanging signals tagged with the same `handshake_id`.
   - Either side may `transport_close` if negotiation fails or when the data channel is torn down.
4. **Host-side handshake manager.** `beach-human` tracks `RemotePeerHandle`s keyed by WebSocket `peer_id`. Each handle owns a map of `{handshake_id -> PeerConnectionState}` so we can service multiple viewers concurrently and retire stale handshakes deterministically.
5. **Ordering & freshness.**
   - Introduce a `GlobalInputSeq` counter on the host. Every time the host applies an input frame it increments the counter and includes the new value in `HostFrame::InputAck { seq, global_seq }` (new field).
   - Clients cache the most recent `global_seq` they have seen. When they send `ClientFrame::Input`, they attach `base_seq` (the `global_seq` the UI snapshot was based on) alongside their monotonic `client_seq`.
   - The host enforces:
     - drop if `client_seq <= per_transport.last_client_seq` (existing guard),
     - drop if `base_seq < global_input_seq` (i.e., another peer already mutated the PTY state after this client’s view),
     - drop if the transport’s `handshake_id` is no longer active for that peer.
   - Accepted inputs advance `global_input_seq`, guaranteeing a total order across peers with O(1) comparisons.
6. **Back-pressure without locks.** The host fans out PTY updates via per-transport synchronizers (already in place). By ignoring stale inputs early we avoid growing queues and keep the data channel hot path branch-light.

## Wire Format Changes
### Client → Server (`ClientMessage` over WebSocket)
```jsonc
{
  "type": "transport_propose",
  "to_peer": "<peer_id>",
  "handshake_id": "<uuid>",
  "transport": { "kind": "webrtc" }
}
{
  "type": "transport_accept",
  "to_peer": "<peer_id>",
  "handshake_id": "<uuid>",
  "transport": { "kind": "webrtc" }
}
{
  "type": "transport_close",
  "to_peer": "<peer_id>",
  "handshake_id": "<uuid>",
  "reason": "<optional human string>"
}
{
  "type": "signal",
  "to_peer": "<peer_id>",
  "handshake_id": "<uuid>",
  "signal": {
    "transport": "webrtc",
    "signal": { ...existing offer/answer/ice payload... }
  }
}
```

### Server → Client (`ServerMessage` over WebSocket)
```jsonc
{
  "type": "transport_proposed",
  "from_peer": "<peer_id>",
  "handshake_id": "<uuid>",
  "transport": { "kind": "webrtc" }
}
{
  "type": "transport_accepted",
  "from_peer": "<peer_id>",
  "handshake_id": "<uuid>",
  "transport": { "kind": "webrtc" }
}
{
  "type": "transport_closed",
  "from_peer": "<peer_id>",
  "handshake_id": "<uuid>",
  "reason": "<optional>"
}
{
  "type": "signal",
  "from_peer": "<peer_id>",
  "handshake_id": "<uuid>",
  "signal": { ... }
}
```

> Note: `join_success` / `peer_joined` remain unchanged. They simply enumerate peers so the host knows whom it can propose to.

`handshake_id` is required for transport-scoped signaling (WebRTC/WebTransport). Diagnostic or custom signals (e.g. CLI debug helpers) may omit it, in which case beach-road bypasses the handshake routing and forwards the payload verbatim.

### Data Channel Payload (`ClientFrame::Input`)
```rust
ClientFrame::Input {
    seq: u32,          // existing per-transport monotonic counter
    base_seq: u64,     // new: last Host global_input_seq observed by the client
    data: Vec<u8>,
}
```
Returns:
```rust
HostFrame::InputAck {
    seq: u32,
    global_seq: u64,   // new: authoritative order index
}
```
Encoding changes propagate to the binary codec (`apps/beach-human/src/protocol/wire.rs`, `apps/beach-web/src/protocol/wire.ts`). Older clients that omit `base_seq` will be rejected during feature negotiation (handled via `protocol_version` bump).

## Component Responsibilities
### beach-road
- Maintain `SessionState.handshakes: HashMap<(peer_id, handshake_id), HandshakeMeta>` containing the destination peer and metadata.
- Route messages strictly by `(session_id, to_peer, handshake_id)` without trying to interpret SDP.
- Garbage-collect `HandshakeMeta` when either peer disconnects or emits `transport_close`.
- Keep REST endpoints enabled but mark them deprecated; once all clients migrate we can remove.

### beach-human (host)
- Extend `SignalingClient`:
  - Track peers in a `DashMap<String, RemotePeerState>` where each state owns `active_handshake: Option<HandshakeId>` plus a bounded queue of incoming WebRTC signals.
  - Expose an API `begin_handshake(peer_id) -> HandshakeChannels` that returns outbound senders + inbound receivers tied to a handshake id.
  - Demote the legacy single `remote_peer`/`remote_generation` logic.
- Update `connect_offerer` to:
  - Request/await a specific `RemotePeerState`.
  - Spawn WebRTC tasks bound to that handshake.
  - Treat `transport_closed` or peer disconnect as a signal to abort and optionally retry with a new handshake.
- Update input pipeline (`spawn_input_listener`): record `global_input_seq` (AtomicU64 shared across transports), enforce the three drop rules, and echo `global_seq` back in `InputAck`.
- Ensure rejects are silent (just drop) so we avoid extra host frames. The UI will quickly resync because clients refresh their `base_seq` on the next host frame.

### beach-web (browser) & CLI joiners
- Persist the latest `global_seq` from `HostFrame::InputAck` (and from initial snapshot/hello which we augment with the current value).
- When sending input, fill the new `base_seq` field. If the browser receives `transport_closed` for its handshake, it tears down the RTC connection and awaits a new proposal.
- Update the signaling client to understand the richer message set and to manage multiple concurrent handshakes (important for future multi-tab support).

## Ordering Semantics (Drawing from Mosh)
- Mosh associates every datagram with a pair `(client_seq, server_epoch)`; older datagrams are dropped because their `server_epoch` trails the current authoritative value.
- We adopt the same strategy: `base_seq` is effectively `server_epoch`. Because PTY state is a pure function of the ordered input stream, discarding stale inputs simply means the lagging client will receive the converged state from the host and can resend with an up-to-date base.
- This scheme is **O(1)** per packet, lock-free (just atomic loads), and robust to network reordering—exactly why Mosh chose it.

## Implementation Checklist
1. **Protocol & codec updates.**
   - Modify Rust/TS enums to introduce `handshake_id` and the new frame fields.
   - Bump the protocol version constant.
2. **Server routing layer.**
   - Extend `ClientMessage`/`ServerMessage` types and update `websocket.rs` dispatch.
   - Add handshake map + cleanup logic.
3. **Host signaling refactor.**
   - Replace single-remote tracking with `RemotePeerState`.
   - Surface async helpers to spawn offerer/answerer negotiations per peer.
4. **Host transport management.**
   - Tie each `SharedTransport` to its originating `handshake_id`.
   - Invalidate transports when `transport_closed` or disconnect events arrive.
5. **Input ordering enforcement.**
   - Add shared `AtomicU64` for `global_input_seq`.
   - Update `ClientFrame::Input` handling + ack emission.
   - Ensure synchronizer snapshots include the latest `global_seq` so late joiners start fresh.
6. **Client updates (browser & CLI).**
   - Track `global_seq`, populate `base_seq` on input frames.
   - Handle new signaling messages and handshake retries.
7. **Testing.**
   - Unit tests for codec compat on both Rust and TS sides.
   - Integration test: spin up host + two clients, ensure second client’s stale input is dropped (no PTY mutation, ack gap observed).
   - Load test script (optional) to open >10 handshakes to validate routing.

## Rollout Considerations
- Feature-flag the new signaling path to allow gradual rollout (CLI env `BEACH_SIGNALING_V2=1`, web query param). Once both host and clients are updated, flip the default.
- Log warnings when legacy REST polling endpoints are hit to catch stragglers.
- Telemetry: count dropped inputs due to stale `base_seq` to spot aggressive concurrency.

## Open Questions / Future Work
- Whether to support renegotiation over an existing data channel (ICE restart) vs. always allocating new `handshake_id`.
- Backwards compatibility window: if we must interoperate with older clients temporarily, we may need a shim that synthesizes `base_seq = current_global_seq` for them (effectively disabling cross-peer protection).

With this blueprint we can build a deterministic, high-performance multi-peer signaling stack that mirrors Mosh’s proven ordering strategy while leveraging WebRTC for transport.

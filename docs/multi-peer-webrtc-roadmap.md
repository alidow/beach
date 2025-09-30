# Multi-Peer WebRTC Roadmap – 2025-09-30

## Context
- `beach-human` currently negotiates WebRTC as the *sole* offerer. When multiple viewers join a session, the host repeatedly reuses a single `remote_peer`, so only one viewer succeeds and the rest see `/webrtc/answer` 404s.
- `beach-road` simply relays WebSocket messages and stores SDP blobs. With the single-remote host, it accumulates stale peer IDs and is spammed by infinite polling of `/webrtc/answer?handshake_id=…`.
- The browser (beach-web) expects an offer targeted at its `peer_id`. Once the host retargets another viewer, the browser loops endlessly waiting for its answer.
- Goal: keep **100 simultaneous viewers** connected to one host via WebRTC data channels without tearing each other down.

## Problems to Solve
1. **Single Remote Pointer** – `SignalingClient` exposes one `remote_peer`. We need per-peer state.
2. **Shared Transport** – The host’s `TransportSupervisor` drives one transport. We need a broadcast fan-out so every viewer sees updates.
3. **Handshake Contention** – Offers, answers, and ICE candidates are interleaved for multiple peers; today the host conflates them.
4. **Runaway Polling** – Missing answers lead to thousands of `/webrtc/answer` 404s. Per-peer handshakes must stop polling once the viewer disconnects.
5. **Observability** – Without structured logs we cannot debug 100 viewers.

## Target Architecture
### 1. Per-Peer Signaling
- Extend `SignalingClient` to store a `HashMap<peer_id, PeerSession>` containing:
  - `generation` / `handshake_id`
  - `mpsc::UnboundedSender<WebRTCSignal>` for ICE/SDP
  - optional “locked” flag while a handshake task is active
- Emit `RemotePeerEvent::{Joined,Left}` so higher layers react to peer lifecycle events.
- Route incoming signals to the correct peer queue and expose a helper `send_signal_to_peer(peer_id, WebRTCSignal)`.

### 2. Offerer Supervisor
- Replace the legacy `connect_offerer_once` loop with an async manager:
  ```
  while let Some(event) = remote_events.recv().await {
      match event {
          Joined(peer) => spawn negotiator task,
          Left(peer_id) => tear down transport,
      }
  }
  ```
- Each negotiator task should:
  1. Build a fresh `RTCPeerConnection`/`RTCDataChannel`.
  2. Post an SDP offer targeted at that peer ID.
  3. Poll `/webrtc/answer?handshake_id=…` until an answer arrives (respect timeouts).
  4. Forward ICE candidates both ways through the per-peer channel.
  5. Produce an `Arc<dyn Transport>` ready for broadcast; hand it to the supervisor.
  6. Close cleanly (abort polling, cancel tasks, remove from supervisor) when the peer disconnects or times out.

### 3. MultiPeerTransport
- Implement a `Transport` wrapper that maintains `HashMap<peer_id, ChildTransport>`.
- Outbound: broadcast frames to every child transport. If `send` returns `ChannelClosed`, remove the peer.
- Inbound: each child transport gets a background task that forwards `TransportMessage`s into a shared crossbeam channel consumed by the host.
- Provide metrics (active peers, buffered amounts) for monitoring.

### 4. Resource Management
- Track per-peer buffered amount; drop peers that exceed thresholds (protect host from slow viewers).
- Send heartbeat/keepalive per peer; remove peers that stop responding.
- Optionally impose a hard cap (e.g. 120 peers) and drop oldest when capacity is reached.

### 5. Testing & Tooling
- Update `apps/beach-human/tests/webrtc_transport.rs` to spawn N dummy answerers concurrently.
- Ensure each answerer receives its offer, posts an answer, and gets broadcast frames.
- Add chaos tests: random join/leave, delayed answers, forced disconnects.

### 6. Logging & Observability
- Include `session_id`, `peer_id`, `handshake_id`, `generation` in key logs.
- Add counters/metrics for active peers, offers posted, answers received, 404 polls avoided.

## Implementation Order
1. **SignalingClient refactor** – per-peer events and routing.
2. **MultiPeerTransport skeleton** – static broadcast with manual insertion/removal (for single peer it should behave exactly like current transport).
3. **Offerer supervisor** – supply per-peer transports to `MultiPeerTransport`.
4. **Wire into `TransportSupervisor`** – replace old transport with the new fan-out.
5. **Enhance tests & logging**.
6. **Benchmark / load test** – simulate 100 viewers and ensure CPU/memory/latency targets are acceptable.

## Acceptance Criteria
- Host maintains 100 simultaneous WebRTC data channels without regressions.
- No viewer triggers endless `/webrtc/answer` 404 polling after the change.
- Integration tests hammering dozens of peers pass within the 60 s timeout budget.
- No feature flags; new architecture is default.

---
*This document captures the architecture and work plan so future Codex runs can implement the multi-peer WebRTC host without rediscovering the failure modes we are seeing today.*

## Reality Check – 2025-10-01
- `SignalingClient` already fan-outs ICE/SDP into per-peer queues (`peer_channels`) and publishes lifecycle events, but the offerer still blocks on `wait_for_remote_peer_with_generation` and aborts if *any* other peer joins mid-handshake (`apps/beach-human/src/transport/webrtc/mod.rs:640`).
- `connect_offerer_once` builds one `RTCPeerConnection`, pumps `/webrtc/offer`/`/webrtc/answer`, and aborts whenever `remote_generation` changes. In practice, the first joiner wins; everyone else hits endless `/webrtc/answer` 404s because the host keeps tearing down the handshake before they receive an answer.
- The host runtime (`SharedTransport`, `TransportSupervisor`, per-sink broadcast loops) already supports many simultaneous transports. The missing piece is a handshake supervisor that can *safely* negotiate multiple viewers in parallel and hand each finished `WebRtcTransport` back to the runtime.
- Without that supervisor, Beach Road still bears the load of repeated `/webrtc/offer` posts and answer polling retries, even though the peers would happily switch to data channels once established.

## Revised Implementation Blueprint
1. **Offerer Supervisor**
   - Build an `OffererSupervisor` that consumes `SignalingClient::remote_events()` and spawns a `PeerNegotiator` task per `RemotePeerJoined`.
   - Each negotiator manages its own `RTCPeerConnection`/`RTCDataChannel`, posts the offer via `/webrtc/offer`, and forwards ICE through `send_signal_to_peer(peer_id, …)`.
   - When a negotiator succeeds, it returns an `Arc<dyn Transport>` plus metadata through a channel; the supervisor exposes `accept_next()` so the host can await the next ready transport.

2. **Cancellation & Cleanup**
   - Track negotiators in a registry so `RemotePeerEvent::Left` or explicit timeouts can abort in-flight handshakes.
   - Ensure `SignalingClient::send_ice_candidate` always targets the correct `peer_id`; drop buffered ICE once the peer disconnects to avoid leaking memory.

3. **Resource Management**
   - Impose per-peer timeouts (offer creation, answer wait, data-channel ready) and cap concurrent negotiators (e.g. 128) to shield the host from stampedes.
   - Once a transport is live, stop polling `/webrtc/answer` and rely on the established data channel for heartbeats.

4. **Observability**
   - Emit structured logs for peer lifecycle (`session_id`, `peer_id`, `handshake_id`, states) plus counters for offers posted, answers received, retries, and abandonments.
   - Surface metrics/hooks so load tests can assert we maintain 100 viewers with minimal Beach Road chatter.

5. **Testing**
   - Extend `apps/beach-human/tests/webrtc_transport.rs` to spin up many concurrent answerers, exercise overlapping joins, and assert every client receives its dedicated offer/transport without `/webrtc/answer` 404 loops.
   - Add failure-path tests (late answers, disconnect during ICE, supervisor cap reached).

## Updated Work Breakdown
1. Implement `OffererSupervisor` + peer negotiator tasks and refactor `connect_offerer` to delegate to it.
2. Wire the supervisor into the host accept loop so existing runtime code receives per-viewer transports without modification.
3. Add timeouts, caps, and cleanup for cancelled negotiators.
4. Instrument logging/metrics for lifecycle + retries.
5. Extend integration tests for concurrent viewers and failure cases.

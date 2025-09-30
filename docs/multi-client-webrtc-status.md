# Multi-Client WebRTC Status – 2025-09-30

## Context
- `beach-human` operates as the WebRTC offerer (the host). Any number of viewers—CLI or browser—may join the same session.
- `beach-road` fans out WebSocket signaling and exposes REST endpoints (`/webrtc/offer`, `/webrtc/answer`) that peers poll while handshaking.
- We must support **100 simultaneous WebRTC viewers** without races, stalled handshakes, or runaway polling.

## Current Status
- **Single-remote bottleneck.** The live code still negotiates exactly one remote peer at a time. When a new viewer joins, the existing data channel stays alive and the newcomer sees endless `/webrtc/answer` 404s.
- **Browser stalls, CLI succeeds.** The first client (often the Rust CLI) completes negotiation. Subsequent viewers fetch the offer but never receive an answer targeted at their `peer_id`.
- **Signaling spam.** Because the host tears down and re-joins while chasing different viewers, Beach Road accumulates dozens of orphaned peer IDs and serves a continuous stream of 404 answer polls.
- **No multi-peer transport.** Outbound frames are pushed to a single transport; there is no fan-out or per-peer flow control.

## Target Architecture (to implement now)
1. **Per-peer signaling sessions** – Track every remote peer separately inside `SignalingClient`, exposing a `RemotePeerEvent::{Joined,Left}` plus a dedicated channel of `WebRTCSignal`s for each peer.
2. **Offerer manager** – Replace the legacy `connect_offerer_once` loop with a supervisor that listens for peer events, spawns a handshake task per peer, and tears it down cleanly when the peer leaves.
3. **Multi-peer transport fan-out** – Implement `MultiPeerTransport` (a `Transport` implementation) that broadcasts outbound frames to every active viewer and forwards inbound frames from each viewer into a shared queue for the host.
4. **Resource management** – Detect slow/broken viewers (buffered amount, missing heartbeats) and remove their transports so 100 loyal viewers stay healthy.
5. **End-to-end coverage** – Update integration tests to exercise dozens of parallel viewers, ensuring each gets an offer, posts an answer, receives broadcast frames, and cleans up on disconnect.

## Implementation Tasks
1. **Signaling (`apps/beach-human/src/transport/webrtc/signaling.rs`)**
   - Store per-peer channels.
   - Emit `RemotePeerEvent`s and route incoming SDP/ICE to the correct peer.
   - Provide helpers to send SDP/ICE to a specific peer ID.
2. **Transport (`apps/beach-human/src/transport/webrtc/mod.rs`)**
   - Add `MultiPeerTransport` plus the supervisor that manages handshake tasks.
   - Rewrite the offerer handshake so each viewer gets its own `WebRtcTransport`.
   - Keep the answerer code path intact for viewers.
3. **Host wiring (`apps/beach-human/src/main.rs`)**
   - Ensure `TransportSupervisor`, heartbeats, and failover logic operate on `MultiPeerTransport`.
4. **Testing (`apps/beach-human/tests/webrtc_transport.rs`)**
   - Add multi-viewer integration tests; guard existing single-viewer tests.
5. **Docs & telemetry**
   - Maintain this status page.
   - Emit structured logs (session, peer_id, handshake_id, generation) for debugging 100-viewer runs.

## Acceptance Criteria
- Host keeps 100 simultaneous `WebRtcTransport` children alive and broadcasting.
- Beach Road no longer sees infinite `/webrtc/answer` 404 loops once multiple viewers join.
- Integration tests (≤60 s each) confirm multiple parallel handshakes succeed and transports clean up after peers leave.
- No feature flags—the multi-peer architecture is the default path.

## Runbook (after implementation)
```bash
cargo test -p beach-human --test webrtc_transport
pnpm --filter beach-web test -- --runInBand

# Manual smoke
cargo run -p beach-road
cargo run -p beach-human -- --session-server http://127.0.0.1:8080
# Join from multiple browsers/CLIs and observe broadcast
```

## Historical Context
Older logs that showcased the single-remote limitation and endless `/webrtc/answer` 404 loops remain under `~/beach-debug/` for reference.

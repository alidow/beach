# WebRTC Dual-Channel Status and Plan

Status: Active  
Date: 2025-09-09  
Owner: Beach Dev Team

## Executive Summary

We have WebSocket signaling and message relay working against beach-road, and a functional WebRTC transport skeleton with both local and remote signaling paths. The subscription pipeline (Subscribe → Ack → Snapshot → Deltas) is implemented, but clients currently hang because WebRTC establishment and/or routing is not consistently completing, so the expected SubscriptionAck/Snapshot never reaches the client. Our immediate goal is to make single-channel WebRTC robust end-to-end, then enable dual data channels (reliable control + unreliable output) and route messages accordingly.

This document summarizes the current state, the specific challenges, and a concrete, file-by-file plan to reach bidirectional dual-channel WebRTC communication with clear diagnostics and fail-fast semantics.

## Current State

- Signaling via WebSocket (beach-road):
  - Server: `apps/beach-road/src/websocket.rs`
  - Messages: `apps/beach-road/src/signaling.rs`
  - Routes: `GET /ws/:session_id` for signaling; HTTP endpoints for session register/join.

- Beach client/server transport abstractions:
  - Transport trait: `apps/beach/src/transport/mod.rs`
  - Channel abstractions: `apps/beach/src/transport/channel.rs` (ChannelPurpose, ChannelReliability, TransportChannel)
  - WebSocket transport: `apps/beach/src/transport/websocket/`
  - WebRTC transport: `apps/beach/src/transport/webrtc/mod.rs`
  - WebRTC remote signaling glue: `apps/beach/src/transport/webrtc/remote_signaling.rs`

- Session + Handlers:
  - Session server side: `apps/beach/src/session/mod.rs` (ServerSession, ClientSession)
  - Signaling adapter: `apps/beach/src/session/signaling_transport.rs`
  - Server message handlers and subscription bridge:
    - `apps/beach/src/session/message_handlers/` (CompositeHandler, SessionBridgeHandler, SubscriptionHandler)
    - Subscription engine: `apps/beach/src/subscription/manager.rs`

- Protocol shape:
  - AppMessage uses a non-flattened `Protocol { message: serde_json::Value }` envelope (correct for routing):
    - `apps/beach/src/protocol/signaling/mod.rs`
  - Typed WebRTC signals exist on the Beach side (`TransportSignal`, `WebRTCSignal`) mirroring beach-road shapes.

- WebRTC status:
  - Server initiates WebRTC as offerer on `PeerJoined`.
  - Client prepares signaling on `JoinSuccess` and answers when an Offer signal arrives.
  - Chunking for large messages exists; legacy single data channel path is present.
  - Dual-channel creation/mapping exists in APIs, but receiving-side mapping and routing to control/output channels are not fully wired for end-to-end flows.

## Current Challenge

- Clients hang waiting for `SubscriptionAck`/`Snapshot` because transport delivery doesn’t complete when Subscribe is sent:
  - WebRTC handshake (Offer/Answer/ICE) not consistently completing or not ready when Subscribe is sent.
  - Data still flows via WebSocket in key paths; strict WebRTC mode exists but isn’t fully enforced in send paths.
  - No client-side timeouts for SubscriptionAck/Snapshot yet; hangs surface as indefinite waits.
  - Multi-channel mapping for WebRTC is not fully integrated (both creation and `on_data_channel` mapping by label), so even when connected, routing isn’t guaranteed to prefer/reach the intended channels.

## Recent Changes Landed

- Client answerer logic: Client now waits for an explicit WebRTC Offer signal before initiating as answerer (no premature initiation on JoinSuccess).
- Transport trait: Added default no-op WebRTC hooks (`initiate_webrtc_with_signaling`, `is_webrtc`) and implemented them in WebRTCTransport.
- Handshake timeouts: 30s timeout wrappers added around WebRTC initiation (server and client) with strict-mode panic messages.
- File-based debug logging: Introduced `DebugLogger` and `webrtc_log!` macro; handshake edges now log to the debug file when `--debug-log` and `--verbose` are enabled.

## Gaps To Close (before dual-channel)

1) Enforce strict WebRTC (no WS fallback for data)
- Client `send_to_server` and server `{send_to_client,broadcast_to_clients}` still fall back to WebSocket signaling for app data. In strict mode (`BEACH_STRICT_WEBRTC=true`), these should error instead of falling back.

2) Add client-side timeouts for subscription flow
- After sending Subscribe, add explicit timeouts for `SubscriptionAck` and `Snapshot` (e.g., 3s each) with actionable error texts.

3) Clearer logging hygiene
- Some `eprintln!` remain in session creation paths; move to file-based `DebugLogger` to avoid TUI corruption and keep logs consistent.

4) Prefer WebRTC data send when connected
- Implement actual data-channel send paths for app messages once WebRTC is up, not just signaling. Keep WS for signaling only.

5) Dual-channel wiring
- Create/control/output channels with correct labels and reliabilities; map incoming `on_data_channel` by label and route accordingly.

## Goals

1. Robust single-channel WebRTC end-to-end for the subscription pipeline (no hangs; clear timeouts; actionable errors).
2. Implement and use dual WebRTC data channels:
   - `beach/ctrl/1` (reliable, ordered) for control, input, protocol, acks
   - `beach/term/1` (unreliable, unordered) for output frames (snapshot/delta)
3. Prefer WebRTC data channels for app traffic; allow optional strict mode to fail fast if WebRTC isn’t established (no data fallback).
4. Instrumentation (file-based logs) for handshake and routing, without corrupting the terminal UI.

## Proposed Plan (Claude-ready)

### Phase A — Stabilize Single-Channel WebRTC

1) Confirm Signal JSON shape parity
- Files:
  - `apps/beach/src/protocol/signaling/mod.rs` (TransportSignal, WebRTCSignal)
  - `apps/beach-road/src/signaling.rs` (reference shape)
- Ensure both sides use:
  - `{ "transport": "webrtc", "signal": { "signal_type": "offer|answer|ice_candidate", ... } }`
  - `sdp_mline_index` as `Option<u32>` on both sides
- Add helpers (if missing): `to_value()` and `from_value()` for `TransportSignal`.

2) Ensure client answerer path triggers
- File: `apps/beach/src/session/mod.rs` (ClientSession::start_message_router)
- On `JoinSuccess`: set `server_peer_id`; create `RemoteSignalingChannel`, set remote peer.
- On `ServerMessage::Signal`:
  - If `TransportSignal::WebRTC::Offer`, call `signaling.handle_signal(signal.clone())` then `WebRTCTransport::connect_with_remote_signaling(signaling, false)`.
  - Route `WebRTCSignal::IceCandidate` to `signaling.handle_signal`.

3) Add explicit timeouts with actionable errors (no hangs)
- File: `apps/beach/src/client/terminal_client.rs` (connect_and_subscribe)
  - Wait for `server_peer_id` (e.g., 3s). On timeout: error “No server peer; ensure beach server connected first.”
  - Wait for WebRTC up (e.g., 10s). On timeout (strict mode only): error “WebRTC handshake timeout; verify signaling and ICE.”
  - After sending Subscribe, wait for `SubscriptionAck` (e.g., 3s) and then `Snapshot` (e.g., 3s). On timeout: error with next steps.

4) Optional strict mode (fail fast)
- Files:
  - `apps/beach/src/session/mod.rs`: ClientSession::send_to_server; ServerSession::{send_to_client,broadcast_to_clients}
- Behavior when `BEACH_STRICT_WEBRTC=true`:
  - If WebRTC transport missing or send fails, return error (do not fallback to WebSocket data path) in:
    - Client: `ClientSession::send_to_server`
    - Server: `ServerSession::{send_to_client,broadcast_to_clients}`

5) Add/verify file-based debug logging at handshake and subscription edges
- Files and points:
  - `apps/beach/src/transport/webrtc/mod.rs`:
    - Offer/Answer creation/sending/receiving; ICE send/receive; peer connection state; data channel open/close
  - `apps/beach/src/transport/webrtc/remote_signaling.rs`:
    - Receipt and queueing of `TransportSignal` (Offer/Answer/ICE)
  - `apps/beach/src/client/terminal_client.rs`:
    - “Sent Subscribe”, “Got SubscriptionAck”, “Got Snapshot”
  - `apps/beach/src/session/message_handlers/session_bridge_handler.rs`:
    - “Received Subscribe”, “Sent SubscriptionAck”, “Sent Snapshot”
- Only log to `BEACH_DEBUG_LOG` (never to stdout/stderr) so TUI remains intact.

Acceptance for Phase A:
- With WebRTC enabled, client reliably receives `SubscriptionAck` then `Snapshot`; TUI renders.
- On failures, client exits with clear error (no hang).

### Phase B — Implement Dual-Channel WebRTC

6) Create control/output channels by label; map incoming channels
- File: `apps/beach/src/transport/webrtc/mod.rs`
  - Ensure `create_channel_internal(purpose)` uses labels:
    - Control: `beach/ctrl/1` (reliable, ordered)
    - Output: `beach/term/1` (unreliable, unordered, `max_retransmits: Some(0)`, `ordered: false`)
  - On server: proactively create Control channel; optionally create Output on demand or proactively based on feature flag `BEACH_OUTPUT_UNRELIABLE=true`.
  - On client: in `on_data_channel`, map `dc.label()` → `ChannelPurpose` and store in `self.channels`.

7) Route messages by purpose
- Files:
  - Server-side routing: `apps/beach/src/session/mod.rs`
    - In `send_to_client` / `broadcast_to_clients`, choose channel:
      - Control channel: AppMessage::Protocol, TerminalInput, TerminalResize, Ping/Pong
      - Output channel: ServerMessage::Snapshot, ServerMessage::Delta, ServerMessage::DeltaBatch
    - If Output not available, fallback to Control (unless strict single-channel configured).
  - Client-side routing for sends: `ClientSession::send_to_server` sends control messages through Control channel.

8) Receiving-side demux
- File: `apps/beach/src/transport/webrtc/mod.rs`
  - Support `Transport::recv()` per channel via `TransportChannel::recv()` or provide a demux layer that feeds:
    - Control messages into the existing protocol handlers (AppMessage::Protocol → ClientMessage/ServerMessage)
    - Output frames directly to the rendering pipeline.
- Easiest incremental approach:
  - Keep existing “legacy” single receive loop for Control (Protocol)
  - Add a new receive loop bound to Output channel that parses `ServerMessage::{Snapshot,Delta,DeltaBatch}` and delivers to the client’s handler (e.g., via the existing `server_tx` channel)

Acceptance for Phase B:
- Two WebRTC data channels are open. Control traffic flows on Control; output frames flow on Output.
- Under light packet loss, output frames may drop without affecting input/control delivery.

### Phase C — (Follow-up) Versioning & Resync

9) Add sequence/versioning to output frames and client acking protocol to support resync under lossy output (tracked in separate spec docs). Not required to unblock dual-channel bring-up.

## Test & Verification Plan

- Integration tests (Rust):
  - `tests/integration/webrtc_signaling_test.rs`: offer/answer/ICE exchange succeeds via beach-road
  - `tests/integration/subscribe_roundtrip.rs`: Subscribe → Ack → Snapshot delivered over WebRTC
  - `tests/integration/dual_channel_test.rs`: Control and Output channels both open; routing verified

- Manual verification:
  1) Start beach-road: `RUST_LOG=debug cargo run -p beach-road`
  2) Start server with logs: `BEACH_DEBUG_LOG=/tmp/server.log cargo run -p beach -- bash`
  3) Start client with logs: `BEACH_DEBUG_LOG=/tmp/client.log cargo run -p beach -- --join <host>/<id> --passphrase <code>`
  4) Check `/tmp/*.log` for:
     - Offer/Answer/ICE; “Data channel opened” (Control and Output)
     - Client: “Sent Subscribe” → “Got SubscriptionAck” → “Got Snapshot”
     - Server: “Received Subscribe” → “Sent SubscriptionAck/Snapshot”

- Strict mode (optional):
  - `BEACH_STRICT_WEBRTC=true` to fail fast if WebRTC not established (disables data fallback to WS in send paths).

## Risks & Mitigations

- ICE networking variability on developer machines → Provide localhost-only path (no STUN) when server/client on same host; add clearer diagnostics for ICE candidate flow.
- Confusion between WS signaling and WS data relay → Make strict mode available; add explicit logs that identify which transport carried app traffic.
- Terminal UI corruption → Never write logs to stdout/stderr; use `BEACH_DEBUG_LOG` only.

## Out of Scope (for this iteration)

- Browser/WebView client.
- End-to-end sealed signaling and Noise handshake (tracked separately in security specs).
- Full resync/versioning framework (Phase C).

## Appendix: Pointers

- Transport abstractions: `apps/beach/src/transport/`
- WebRTC transport & signaling: `apps/beach/src/transport/webrtc/`
- Session & handlers: `apps/beach/src/session/`
- Subscription engine: `apps/beach/src/subscription/`
- Protocol: `apps/beach/src/protocol/`

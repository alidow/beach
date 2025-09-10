# Dual‑Channel WebRTC for Terminal Streaming (Mosh‑like)

Status: Draft v1

Owner: beach

Scope: `apps/beach` (server + client), interoperating with `apps/beach-road` as untrusted signaling.

Summary: This document defines a two‑channel WebRTC transport for terminal sharing. A reliable, ordered control channel carries handshake, authorization, keyboard input, acknowledgements, and control messages. A separate unreliable, unordered output channel carries terminal render frames with occasional loss tolerated via periodic resynchronization. The goal is to deliver snappy, resilient output under loss like “mosh”, while preserving correctness of input.

---

## Problem

- Terminal input must never be lost or reordered. Even a single missed byte (e.g., in a password) is unacceptable.
- Terminal output can be lossy if the client periodically resynchronizes with the authoritative server state. Re‑sending a newer frame can be better than waiting to retransmit a stale one under loss.
- A single reliable channel ties output to retransmission and head‑of‑line blocking, producing lag under loss.

## Solution Overview

- Control channel (`beach/ctrl/1`): reliable + ordered.
  - Uses: initial handshake, authorization, keyboard input, viewport changes, pings/heartbeats, acknowledgements, resync requests, and error reporting.
- Output channel (`beach/term/1`): unreliable + unordered.
  - Uses: high‑rate terminal output frames (deltas or tiles). The sender may drop older frames in favor of newer ones.
- Security: Both channels are established after sealed signaling and a successful application handshake on the control channel. All application payloads are protected with the session keys derived in that handshake.

## Data Channel Parameters

- Control channel
  - Ordered: true
  - Reliability: fully reliable (default)
  - Label: `beach/ctrl/1`
- Output channel
  - Ordered: false
  - Reliability: `max_retransmits = 0` (or `max_packet_lifetime` small)
  - Label: `beach/term/1`

Note: Reliability/order are fixed at creation; do not attempt to switch a channel from reliable to unreliable later. Create both channels explicitly.

## Frame Model

- Authoritative state: the server maintains a terminal state tracker and can produce:
  - Snapshots: full viewport state.
  - Deltas: minimal updates since a known version.
- Client render state: the client tracks the last applied version.
- Versioning: each snapshot/delta carries a monotonically increasing `version` (u64). Versions are per‑connection.

### Sender Behavior (Server)

- Maintain a short window of recent deltas keyed by version (e.g., last 2–5 seconds).
- For each render tick or PTY update:
  - If the client ack is recent (within window), send a delta against the client’s last acked version.
  - Else, send a snapshot (or a delta chain trimmed to fit limits).
- Drop queued older frames when a newer delta supersedes them; do not retransmit lost output frames on the unreliable channel.
- Periodically emit a compact hash (e.g., BLAKE2s) of the current viewport on the control channel to detect silent divergence.

### Receiver Behavior (Client)

- Apply deltas in any arrival order, validating `base_version`.
- If a delta does not match the current version, request a resync on the control channel.
- Ack the last applied `version` periodically on the control channel (e.g., every 50–100 ms or N frames).
- If hashes mismatch or too many gaps are detected, request a full snapshot.

## Control Messages

- `Ack { version }`: client → server; last applied version.
- `ResyncRequest { reason }`: client → server; asks for snapshot.
- `Viewport { cols, rows }`: client → server; resize events.
- `Heartbeat { t }` and `HeartbeatAck { t }`: liveness.
- `Hash { version, h }`: server → client; optional integrity beacons.

These ride on the reliable control channel and are protected by application‑level encryption.

## Integration With Existing Terminal State

- Use existing components in `apps/beach/src/server/terminal_state/`:
  - `GridView` to derive current viewport.
  - `GridDelta` to compute per‑tick changes.
  - `GridHistory` to back snapshots/delta windows.
- Map `GridDelta` payloads to output channel frames; include `base_version`, `next_version`.
- Generate snapshots using `GridView` (optionally compressed) when resync is needed.

## Flow

1) Sealed signaling with passphrase (see zero‑trust spec), then establish WebRTC.
2) Create control channel (reliable). When open:
   - Run application handshake; verify authorization.
   - Only on success, proceed.
3) Create output channel (unreliable). When open:
   - Start streaming deltas; service resync requests via snapshots.
4) Maintain periodic ack/heartbeat on control channel; adjust sender strategy based on ack freshness and loss.

## Backpressure and Rate Control

- Output channel: prefer drop‑old in favor of latest; bound send queue length.
- Control channel: enforce small bounded queues; prioritize input and acks.
- Adaptive tick rate: reduce frame rate under sustained loss or when acks lag; increase when healthy.

## Failure and Fallbacks

- If the unreliable channel fails to open, fall back to a single reliable channel mode (reduced performance).
- If divergence persists despite deltas, force a snapshot and possibly throttle until stable.
- Under very high loss, temporarily switch to snapshot‑only until conditions improve.

## Security

- Both channels are under the same session keys from the handshake run on the control channel.
- Include channel labels and roles in the handshake transcript to prevent cross‑channel confusion.
- No secrets or tokens on the output channel; all policy decisions happen on the control channel.

## Configuration (proposed)

- `BEACH_DC_CTRL_LABEL` (default `beach/ctrl/1`)
- `BEACH_DC_TERM_LABEL` (default `beach/term/1`)
- `BEACH_DC_TERM_UNRELIABLE` (default `true`)
- `BEACH_TERM_ACK_INTERVAL_MS` (e.g., `75`)
- `BEACH_TERM_DELTA_WINDOW_MS` (e.g., `3000`)

## Implementation Checklist

- [ ] Add creation of two data channels with correct reliability/order settings.
- [ ] Implement versioned deltas and snapshots on server; maintain recent delta window.
- [ ] Add client‑side version tracking, acks, resync logic, and snapshot application.
- [ ] Define and serialize control messages over the reliable channel.
- [ ] Add adaptive rate/backpressure on server output.
- [ ] Integrate with the zero‑trust handshake and authorization on control channel before enabling output.


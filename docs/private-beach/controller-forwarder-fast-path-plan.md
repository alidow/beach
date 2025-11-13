# Controller Forwarder Fast-Path Plan

## Goal
Make Beach Manager’s controller forwarder deliver actions over the `mgr-actions` WebRTC channel as soon as it becomes available, falling back to HTTP only when the channel never establishes.

## Current Issues
1. `drive_controller_forwarder` still uses the primary (HTTP) transport because `NegotiatedSingle.webrtc_channels` isn’t passed down, so `select_controller_transport` never waits for `mgr-actions`.
2. Even when the host opens `mgr-actions`, the manager keeps queueing actions via Redis/HTTP, causing queues to overflow while hosts pause their HTTP pollers.

## Desired Behavior
- On successful negotiation, the forwarder should:
  1. Wait up to N seconds for `mgr-actions` (and legacy `pb-controller`).
  2. Switch all subsequent `ClientFrame::Input` sends to that channel.
  3. Continue to ACK via HTTP only for bookkeeping.
- If the channel isn’t ready within the timeout, continue using HTTP but retry the fast-path wait whenever a new `webrtc_channels` set appears (e.g., after reconnect).

## Implementation Steps

### 1. Thread WebRTC metadata
1. Update `controller_forwarder_once_with_label` to pass the full `WebRtcChannels` (and connection metadata) into `drive_controller_forwarder`.
2. Ensure `NegotiatedSingle` is imported in Beach Manager (already available via `beach_client_core`).

### 2. Transport selection logic
1. Enhance `select_controller_transport`:
   - Wait for `mgr-actions`, falling back to `pb-controller`.
   - On success, return the `Arc<dyn Transport>` for that channel plus `transport_label="fast_path"`.
   - If wait times out, return the primary HTTP transport but remember the metadata label so we can retry later.
2. Log every switch (HTTP → fast_path, fast_path → HTTP fallback).

### 3. Delivery loop updates
1. Ensure `transport.send_bytes` writes to the fast-path channel when selected.
2. Continue to call `ack_actions(... via_fast_path=true)` so telemetry reflects fast-path usage.
3. When the channel closes, fall back to HTTP and log a warning.

### 4. Tests
1. Unit test `select_controller_transport` with mocked `WebRtcChannels`:
   - Channel available → returns fast-path.
   - Channel errors/timeouts → returns HTTP.
2. Integration test:
   - Spin up a host that exposes `mgr-actions`; assert manager logs `transport="fast_path"` and the host logs `controller.actions.fast_path.apply`.
   - Kill the channel; ensure manager logs fallback to HTTP and resumes fast-path when the channel returns.

### 5. Observability
1. Add Prometheus counters:
   - `controller_fast_path_deliveries_total`
   - `controller_fast_path_fallbacks_total`
2. Add structured logs whenever pending actions are flushed due to channel errors.

### 6. Rollout
1. Feature flag (`CONTROLLER_FAST_PATH_ENABLED`) to guard the new behavior.
2. Enable flag in staging, verify Pong runs end-to-end, then roll into dev.

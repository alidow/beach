# Controller Fast-Path Churn (Nov 2025)

## Problem Statement
The Pong showcase controller connectors between the agent tile and both player tiles still flip between `fast_path` and HTTP fallback. Even after the handshake-ID hints, cached transport updates, and host-side throttling, Beach Manager repeatedly times out while waiting for the controller `mgr-actions` channel and subsequently falls back to `pb-controller`. Browser telemetry (`temp/pong.log`) shows nonstop `/sessions/<id>/webrtc/offer` 404s for the controller and both players, and `temp/pong-stack/agent.log` logs `poller started (fallback)` every few seconds despite intermittent `fast-path ready` events.

## Observations

- **Manager logs** (`logs/beach-manager/beach-manager.log`)
  - `fast path controller channel ready` arrives once per host session (confirming host throttling landed).
  - Every ~15s the controller forwarder logs `timed out waiting for controller data channel` and immediately reconnects via `pb-controller`.
- **Host logs** (`temp/pong-stack/beach-host-*.log`)
  - Only a single `fast path controller channel ready` per attach; HTTP action pollers pause right away.
- **Agent log** (`temp/pong-stack/agent.log`)
  - Alternates between `fast-path ready` and `fast-path unavailable`, proving the connectors really fall back even if the dashboard still displays `fast_path` (because identical status updates are deduped now).
- **Browser console** (`temp/pong.log`)
  - Flood of `/sessions/.../webrtc/offer?... 404` entries for controller + players. This matches the manager rejecting controller offers whenever the expected handshake ID doesn’t match the arriving channel.

## Fixes Attempted

1. **Handshake ID hints** – Manager tags controller auto-attach hints with UUIDs and ignores redundant attach requests with the same `x-beach-handshake-id`. Hosts echo the header. Regression: `controller_auto_attach_handshake_header_prevents_renegotiation`.
2. **Host throttling** – `OffererInner` now gates controller peers (labels `mgr-actions` / `pb-controller`) so only one negotiator runs at a time; additional peers are dropped until the slot frees. Regression: `controller_peer_tracker_limits_parallel_negotiations`.
3. **Transport status dedupe** – `AppState::update_pairing_transport_status` no longer publishes identical `PairingTransportStatus`, eliminating connector spam but not the underlying fallback.
4. **Cached replay** – Manager replays the latest transport status when a controller pairing finally materializes, so UI catches up if the pairing arrived late.

Despite the above, Beach Manager still binds itself to the “latest” handshake ID and ignores the already-established `mgr-actions` channel. When that expected channel never appears (because throttling stopped the redundant renegotiations), the forwarder waits out `CONTROLLER_FAST_PATH_WAIT` (15s), logs the timeout, and falls back to `pb-controller`. This causes:

- Fast-path connectors to bounce every 15s.
- `temp/pong.log` 404s as the browser keeps retrying the rejected offer endpoints.
- The agent oscillating between WebRTC and HTTP.

## New Instrumentation (Nov 18)

- `select_controller_transport` and `wait_for_controller_data_channel` now log the session/private beach IDs whenever we start waiting, detect existing channels, encounter channel errors, or time out. This should reveal which label we were waiting on when the timeout fired and whether any channel had actually been published.
- These logs live under the `controller.forwarder` target so they appear next to the existing `fast-path data channel detected` trace.

After you rerun `pong-stack.sh`, grab the latest `logs/beach-manager/beach-manager.log` and filter for `controller.forwarder` to see the new context.

## Current Blocker

Manager’s controller forwarder still insists on seeing the `mgr-actions` channel produced by the most recent handshake. Now that hosts only create one controller offer at a time, any retry resets the expected handshake and causes the already-open channel to be ignored. The forwarder should simply accept the first `mgr-actions` channel that becomes available and switch transports immediately.

## Proposed Next Steps

1. **Implement “first channel wins”** in `drive_controller_forwarder` / `FastPathUpgradeHandle`:
   - As soon as `WebRtcChannels::publish` produces any `mgr-actions` channel, switch transports regardless of handshake ID ordering.
   - Cache the `Arc<dyn Transport>` so later retries don’t clobber it unless the channel truly dies.
2. **Regression coverage**:
   - Extend `fast_path_watchers_dedupe_back_to_back_channels` to assert we emit exactly one `ForwarderEvent::FastPathReady` even if two channels arrive quickly.
   - Add a unit test that seeds two `mgr-actions` channels in different orders and confirms the watcher resolves to whichever arrives first without timing out.
3. **Replay validation**:
   - Re-run `scripts/fastpath-smoke.sh` (after `direnv allow` to populate `BEACH_ICE_PUBLIC_*`) and a full Pong replication.
   - Ensure no new `timed out waiting for controller data channel` log lines appear and the agent log never restarts the HTTP poller.
4. **Longer-term**:
   - Consider emitting a DevTools timeline event whenever the controller forwarder abandons a fast-path attempt so the dashboard can highlight the cause.
   - Evaluate reducing `CONTROLLER_FAST_PATH_WAIT` once the acceptance fix lands so fallbacks trigger faster if ICE truly fails.

## Hand-off Prompt

When spinning up a second Codex instance, share the latest logs (`temp/pong-stack/*.log`, `logs/beach-manager/beach-manager.log`) plus this document, then ask it:

```
You’re reviewing persistent controller fast-path churn in the Beach Pong demo. Hosts now throttle controller peers and log a single `fast path controller channel ready`, but Beach Manager still logs `timed out waiting for controller data channel` every 15s and the agent flips back to HTTP. Using docs/private-beach/controller-fast-path-churn.md plus the latest `temp/pong-stack` + `logs/beach-manager` artifacts, propose the concrete code changes (with tests) required so the controller forwarder accepts the first `mgr-actions` channel and keeps the connector green.
```

This context should help the new agent reason about the remaining manager-side work without rediscovering the earlier fixes.

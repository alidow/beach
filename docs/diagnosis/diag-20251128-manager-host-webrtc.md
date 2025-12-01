## Context
- Stack restarted via `scripts/dockerup` (LAN/TURN 192.168.68.52:3478), then `pong-stack.sh start bc4c51ba-2004-4e6a-9953-cbf31a83f0f6` with RUST_LOG webrtc trace.
- Host/player/agent logs live inside the manager container at `/tmp/pong-stack/20251128-085811` (lhs session `675c8415-7523-40a4-a6fb-93727d729e55`, rhs `54e4b84c-28af-4a10-9654-1f35ad8c9d61`, agent `0fa327c1-c8d0-4489-8ab4-322288c26021`).
- Frontend `temp/pong.log` shows connector tiles for lhs falling back to `http_fallback` (later in the run).

## Timeline (manager ↔ host)
- 13:58:18Z: Hosts start, register, advertise WebRTC offers (signaling URLs at beach-road:4132). Health/state publishes immediately 404 “session not found”.
- 13:58:38Z: Manager handshake arrives; auto-attach confirmed; controller action consumer starts.
- 13:58:39Z: Hosts try to fetch answers for peer_session_ids:
  - lhs: `peer-sessions/cdd6d588-8a96-40b1-831e-3abdd1864723/webrtc/answer?...` returns 409 (multiple retries).
  - rhs: `peer-sessions/cb8ce39a-5497-4715-b41a-44fba618dfd8/webrtc/answer?...` returns 409 (multiple retries).
- ~13:58:44Z: Manager log shows `webrtc answerer connected … peer_session_id=cdd6d588…` and `cb8ce39a…`; controller forwarder negotiated `transport=WebRtc has_webrtc_channels=true`.
- 13:58:52Z: Host answer fetch for lhs succeeds (200) with handshake_id=849fbd3a…; rhs shows the same pattern. After this, host logs show data channels created, ICE candidates exchanged, and large sync frames sent.
- Frontend timeline at 13:59:52Z still shows connector actions `status:"http_fallback"`—likely a UI indicator mismatch or later degradation, not a failed initial negotiation.

## Browser ↔ host
- No fresh `webrtc-internals` dump tied to these session IDs in the provided files; cannot confirm ICE success for this run. (Prior runs showed host/prflx success, but not applicable here.)

## Updated assessment (manager ↔ host)
- Manager↔host RTC did come up after initial 409s: answers were eventually served, PCs connected, and WebRTC channels negotiated (`has_webrtc_channels=true`). Host logs show sustained datachannel traffic; no disconnects through 14:02 were observed.
- The lingering `http_fallback` in the UI appears to be a mismatch/late indicator rather than a failed negotiation.
- Early 404s on host health/state publishes (“session not found”) still indicate the manager graph wasn’t ready when hosts started, but it recovered once the handshake/attach completed.

## Hypotheses still open
1) UI indicator drift: connector tiles may default to HTTP if fast-path metadata is missing, even when WebRTC is active; needs explicit RTC state reporting to UI.
2) Controller input path empty: host logs show repeated “controller action poll returned empty set”; if agent/player actions aren’t flowing over manager, the game stays inert despite RTC.
3) Startup race: early 404s on health/state publishes hint the session graph isn’t ready; tightening attach/order or retry logic may reduce initial 409s.
4) Auth/config hardening: manager still runs with DEV_MANAGER_INSECURE_TOKEN; supplying a real Gate JWT would remove an auth edge case even though answers succeeded here.

## What to log next run (to close the gap)
- Manager (beach-manager):
  - Emit RTC connection-state and transport used for controller forwarder (session_id, peer_session_id), and surface it to UI.
  - Log controller actions received per session to confirm inputs arrive.
- Host (beach-host-*):
  - Keep answer fetch logging; add ICE/connection state hooks to mark when RTC transitions connected/disconnected.
  - Log counts of controller actions applied.
- Frontend:
  - Log manager-provided `ice_servers` and RTC connection-state for each tile; distinguish fast-path HTTP vs RTC transport explicitly.
  - If possible, surface controller-action throughput to confirm input flow.

## Bottom line
Manager↔host RTC did connect (answers eventually served; manager and host both reported WebRTC channels). The UI still showed HTTP fallback, likely due to indicator drift or later degradation. Game stayed inert because controller actions appear empty. Add explicit RTC state reporting and controller-action logging to align UI with actual transport and to confirm input flow.

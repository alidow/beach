# Pong WebRTC fallback diagnosis (run: 2025-11-28 08:58:11 start)

Contexts examined (all in `/tmp/pong-stack/20251128-085811` inside `beach-manager`):
- Player host logs: `beach-host-lhs.log`, `beach-host-rhs.log`
- Agent log: `agent.log`
- Manager logs: `docker compose logs beach-manager` (notably `drain_actions_redis` warnings)
- Surf client trace: `temp/pong.log`
- ICE / TURN env: `BEACH_ICE_PUBLIC_IP/BEACH_ICE_PUBLIC_HOST` set to `192.168.68.52`; `BEACH_TURN_EXTERNAL_IP` unset (warning on every log read).

## Timeline and symptoms
- 13:58:19–13:58:33 UTC: Both hosts try HTTP publish before registration, receive 404 `session not found` (LHS/RHS logs lines ~77/103).
- 13:58:39–13:58:45 UTC:
  - Agent pairs; controller leases active for LHS `675c8415-...` and RHS `54e4b84c-...`.
  - Agent logs “WebRTC ready” for both, but immediately starts HTTP pollers and logs “allowing HTTP fallback”; state pollers stop right after first snapshot, leaving “waiting for players (transport/state...)”.
- 13:58:38–13:58:55 UTC: Host logs show extensive ICE candidate sealing/decryption with both private (192.168.68.52) and public (68.175.123.188, 172.64.150.54) candidates. No data-channel “connection state” messages appear; no evidence of RTC DC ever opening.
- 13:59:32 UTC: LHS flaps (“WebRTC unavailable ... falling back” then “WebRTC restored”), but agent remains on HTTP poller.
- 14:00–14:14 UTC:
  - Manager repeatedly warns `drain_actions_redis returned no actions ... rtc_ready=false` for RHS controller delivery queue (queue depths up to 50).
  - Agent continually logs `queue_action failure/timeout` for both hosts and readiness blocked on `state_stale`; action ACKs never arrive.
  - Surf client trace (`temp/pong.log`) shows connector transport as `http_fallback`, with “stalled; treating as stalled” and all actions via HTTP.
- State traces: paddles static; no ball updates, confirming no controller delivery/state streaming after initial snapshots.

## Root cause
The manager↔host RTC data channel never became usable. ICE negotiation exchanged candidates (host logs show decrypted public/private candidates), but no DC open/connected events and manager delivery kept `rtc_ready=false`, forcing HTTP poller fallback. With RTC unavailable, controller actions and state stayed on the degraded HTTP path and timed out. Contributing factors:
- TURN external IP unset (`BEACH_TURN_EXTERNAL_IP` missing) may have limited relay reachability if direct public/lan paths failed.
- Initial 404s (session not found) suggest a short gap before session registration; combined with missing DC connect, the agent quickly marked the connector stalled (`no action acknowledgements >1.5s`) and locked into fallback.
- Env check inside `beach-manager` container (`env`): `BEACH_ICE_PUBLIC_HOST/IP` were set to `192.168.68.52`, but `BEACH_TURN_EXTERNAL_IP` was **unset** (warning emitted on every log read). `BEACH_GATE_TURN_URLS` pointed at `turn:192.168.68.52:3478`. If the browser/manager and hosts weren’t on the same LAN, relay traffic likely failed, explaining why ICE exchanged candidates but the DC never connected. No DC “connection state” logs appear in host files to contradict this.

## What to log next run (to pin exact DC failure)
- Host side: add DEBUG logs for WebRTC peer/ICE/DC state changes (connection/iceConnection state, data-channel open/close) and for controller channel send/ack counts.
- Manager side: log controller delivery path reasons when `rtc_ready=false` (e.g., ICE/DC state, last error) and when draining actions to HTTP poller instead of RTC.
- Agent side: log first action ACK latency and explicit connector stall reason per session.
- Network: capture STUN/TURN reachability and selected candidate pair (type, protocol) on host and manager.

## Summary
Only one RTC attempt per host occurred; it advertised candidates but never reached a stable data channel. Manager kept `rtc_ready=false`, so controller delivery fell back to HTTP pollers, causing timeouts and no game state/controls. Ensure TURN/public hints are set (`BEACH_TURN_EXTERNAL_IP`), add DC/ICE state logging, and verify selected candidate pairs to isolate why the DC never connected.***

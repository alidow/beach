# 2025-11-28 WebRTC fallback diagnosis (bc4c51ba-2004-4e6a-9953-cbf31a83f0f6)

## Scenario
- Stack started via `pong-stack.sh start bc4c51ba-2004-4e6a-9953-cbf31a83f0f6` at ~13:58Z.
- Sessions: lhs `675c8415-7523-40a4-a6fb-93727d729e55`, rhs `54e4b84c-28af-4a10-9654-1f35ad8c9d61`, agent `0fa327c1-c8d0-4489-8ab4-322288c26021`.
- Artifacts:
  - Host logs: `temp/beach-host-lhs-20251128.log`, `temp/beach-host-rhs-20251128.log` (copied from `beach-manager:/tmp/pong-stack/20251128-085811/`).
  - Browser console: `temp/pong.log`.
  - WebRTC internals: `temp/webrtc-internals.log` (not needed for timelines; no contradictory evidence).
  - Manager logs: `temp/docker-beach-manager-20251128.log`.

## Timeline — Browser ↔ Host (lhs session 675c8415…)
- 13:58:51.551Z host sees dashboard peer join from `172.64.150.54:37197` (peer_id `250d1199…`, label `private-beach-dashboard`). `temp/beach-host-lhs-20251128.log:978`
- ICE candidates received from browser:
  - srflx: `172.64.150.54:42169` (multiple entries). `...log:1055/1270/1346`
  - “private”: `192.168.68.52:63014` (host’s NAT hint published outward). `...log:1060/1275/1351`
  - No relay candidates.
- 13:58:53.233Z primary data channel opens; secure/ready handshakes succeed; viewer registered. `...log:1091–1142`
- 13:59:52.68Z browser connector switches to `status/transport:"http_fallback"` (first console ack). `temp/pong.log:11`
- 14:00:09.208Z host logs “peer left” for dashboard peer. `...log:11205`
- Net: Browser WebRTC succeeded, then dropped ~1m later; browser fell back to HTTP before the host logged the leave. With only srflx/no relay, the link depended on hairpin; likely the NAT binding expired or path flapped.

## Timeline — Manager ↔ Host (lhs session 675c8415…)
- 13:58:38.656Z manager viewer join (peer_id `aac056a6…`, peer_session_id `cdd6d588…`). `...log:136`
- 13:58:43.530Z manager viewer data channel opens. `...log:395`
- 13:58:44.63Z readiness completes; controller channel ready on WebRTC transport_id=1. `...log:486–547`
- Manager ICE shows only private + srflx candidates, no relay:
  - Private: `192.168.68.52:62551` (viewer) / `192.168.68.52:62517` (actions). `...log:328–329`
  - Srflx: `68.175.123.188:53133/53119`. `...log:340–341`
- Manager logs: ICE `Connected` at 13:58:41.87Z; transport `webrtc` established at 13:58:44.526Z; transport status set to `Rtc` 13:58:44.78Z. `temp/docker-beach-manager-20251128.log:736/741/1259/1324`
- Starting 13:59:53Z manager warns `rtc_ready=false` while draining actions; by 13:59:57Z controller forwarder is dispatching over `transport="http_fallback"` (continues through 14:00+). `temp/docker-beach-manager-20251128.log:14830–19457`
- Host logs show no “peer left” or channel close for manager peers; WebRTC channels remain registered.
- Net: Manager WebRTC connected and stayed open per host logs, but manager’s controller forwarder declared RTC not ready ~1 min later and fell back to HTTP despite the channels remaining up.

## RHS host (54e4b84c…)
- Similar pattern: manager viewer/actions connected via WebRTC at 13:58:44.5Z (`temp/docker-beach-manager-20251128.log:1258/1262/1276`); later controller forwarder also switches to `http_fallback` with `rtc_ready=false` warnings (`...log:14845,16742,16899,19457`).

## Probable causes
1) **Browser drop (root cause likely missing/invalid TURN):**
   - Browser only provided srflx + a “private” candidate derived from host’s NAT hint; no relay candidates.
   - Env at manager start: `BEACH_GATE_TURN_URLS=turn:192.168.68.52:3478`, **`BEACH_TURN_EXTERNAL_IP` unset** (manager log attempt block shows it as `None`). Coturn would advertise an empty external IP → relayed candidates suppressed/invalid.
   - With no relay, the browser depended on a hairpin srflx path; it worked briefly, then died ~60s later.

2) **Manager fallback to HTTP (root cause still unclear, but RTC marked unready):**
   - Actions + viewer WebRTC channels opened and never logged as closed on host.
   - Manager repeatedly sets `rtc_ready=false` in controller delivery and reroutes actions over HTTP.
   - ICE also lacked relay (same missing `BEACH_TURN_EXTERNAL_IP`), so P2P relies on srflx/private. However, host and manager are on the same Docker network; relay shouldn’t be required, so absence of TURN alone doesn’t fully explain `rtc_ready=false`.
   - Possible triggers (need confirmation):
     - Data-channel acks not observed by manager (buffered/send errors not logged).
     - Some health/heartbeat timeout inside controller forwarder despite channel open.
     - Manager bearer missing warning during negotiation (`manager bearer token missing; WebRTC peer auth may fail`) could downgrade capabilities or skip fast-path heartbeats.

## Gaps & logging to add for next run
To conclusively pin the manager fallback cause:
- **Log selected ICE pair and transport state transitions** on both host and manager (local/remote candidate, type, network type, relay/protocol).
- **Log controller forwarder RTC readiness transitions** with reasons: current transport_id/peer_id, data-channel state, last received/sent timestamp, buffered_amount, and any send errors.
- **Log data channel close/error events** on host (per peer_id/label) and manager, not just peer-left.
- **Log TURN configuration actually applied** (ICE servers list) on both host and manager at negotiation start.
- **Capture controller forwarder path choice** when switching to HTTP: include session_id, peer_id, trigger (timeout? ack backlog? rtc_ready=false?), and current ICE/PC state snapshot.

Configuration fixes to try in parallel:
- Set `BEACH_TURN_EXTERNAL_IP=<LAN/HOST IP>` so coturn emits valid relay candidates; ensure `BEACH_GATE_TURN_URLS` points to the same host IP for all services.
- After setting, re-run and re-collect host/manager logs to see if relay candidates appear and whether manager stays on RTC.

## Summary
- Browser ↔ host WebRTC succeeded, then dropped after ~1 minute; no relay candidates were available, pointing to incomplete TURN config (`BEACH_TURN_EXTERNAL_IP` unset) and hairpin fragility.
- Manager ↔ host WebRTC established and stayed open per host logs, but manager declared `rtc_ready=false` and fell back to HTTP for actions after ~1 minute. Root cause not visible in current logs; additional logging (ICE pair selection, RTC readiness reasons, channel errors, and applied ICE server config) is needed to confirm whether this is a connectivity issue (no relay) or a forwarder health/heartbeat bug. 

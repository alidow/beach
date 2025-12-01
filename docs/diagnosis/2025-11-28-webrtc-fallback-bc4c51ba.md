## Context
- Run: 2025-11-28, beach `bc4c51ba-2004-4e6a-9953-cbf31a83f0f6`
- Sessions: lhs `675c8415-7523-40a4-a6fb-93727d729e55`, rhs `54e4b84c-28af-4a10-9654-1f35ad8c9d61`, agent `0fa327c1-c8d0-4489-8ab4-322288c26021`
- Logs inspected: browser console `temp/pong.log`; WebRTC internals `temp/webrtc-internals/webrtc_internals_dump (3).txt`; host logs `/tmp/beach-host-{lhs,rhs,agent}-20251128.log` (copied from container); agent/player logs `/tmp/agent-20251128.log`, `/tmp/player-{lhs,rhs}-20251128.log`

## Timeline (UTC)
- 13:58:18–19Z lhs host starts, negotiates WebRTC offer to road.
- 13:58:38Z manager viewer + mgr-actions join lhs (`beach-host-lhs-20251128.log:136,154`).
- 13:58:38Z manager viewer + mgr-actions join rhs (`beach-host-rhs-20251128.log:134,154`).
- 13:58:38–39Z manager viewer + mgr-actions join agent (`beach-host-agent-20251128.log:35,53`).
- 13:58:39Z agent log: “allowing HTTP fallback” for rhs and lhs; state pollers started, even though WebRTC “ready” messages follow at 13:58:44–45Z (`/tmp/agent-20251128.log`).
- 13:58:44–45Z agent reports WebRTC ready for rhs then lhs, yet immediately restarts HTTP pollers (“allowing HTTP fallback…”).
- 13:58:51Z browser peer joins lhs (peer_id 250d1199…).
- 13:59:01Z browser peer joins rhs (peer_id 95d6974e…).
- 13:59:18Z browser peer joins agent (peer_id d510efed…).
- 13:59:52–13:59:59Z browser connector marks all controller actions as `transport:"http_fallback"`, then “no action acknowledgements for >1.5s; treating as stalled” (lhs at 13:59:59.176Z) in `temp/pong.log`.
- 14:00:09Z browser peers for lhs/rhs leave (`beach-host-lhs-20251128.log:11205`, `beach-host-rhs-20251128.log:13133`).
- 14:00:39Z browser peer for agent leaves (`beach-host-agent-20251128.log:160`).
- Manager peers never leave; manager↔host RTC stays up throughout.

## Transport details
- WebRTC internals (viewer): selected candidate pair is prflx→host (local prflx, remote host 192.168.68.52:62527), RTT ~1–3 ms. TURN configured but not used.
- Manager↔host: stable WebRTC (no peer-left). Datachannel traffic visible in host logs after joins.
- Browser connector: flips to HTTP fallback before disconnect; action path stalls/ACKs missing.

## Symptoms
- Connector wires turn green in UI (~1 min in), matching browser logs of HTTP fallback and ACK stall.
- Agent log shows HTTP fallback allowed immediately after startup despite “WebRTC ready”; state pollers started/stopped repeatedly.
- No evidence manager↔host RTC dropped; the failure is on the controller/data delivery path from browser side (actions/ACKs stall).

## Likely cause
- Controller delivery/ACK path from browser stalls while the RTC socket is still connected. Browser sees no ACKs, marks the channel stalled, falls back to HTTP, and later the browser peers close. Manager↔host RTC remains healthy (both are on the same Docker network and do not need TURN), so the stall is likely between browser↔manager or manager’s forwarding/ACK handling, not physical ICE failure on manager↔host.
- Agent log enabling HTTP fallback immediately after “WebRTC ready” suggests manager never considers the controller link “ready” (possibly missing ACKs/handshake or layout/controller mapping issues), so it enables HTTP pollers from the start.
- Networking angle worth testing for browser↔host: browser remote_addr in host logs is a Cloudflare/WARP IP (172.64.150.54). With VPN/WARP active, srflx/host pairs to 192.168.68.52 (LAN IP) may be hairpinned or filtered; TURN is configured to the LAN IP but not forced. This could intermittently stall browser datachannels while manager↔host stays up. Forcing TURN with a reachable public IP would clarify.
- Infra/env confirmed in containers: BEACH_ICE_PUBLIC_HOST=192.168.68.52, TURN URLs turn:192.168.68.52:3478, surfer has NEXT_PUBLIC_FORCE_TURN=0. If the browser is off-LAN/VPNed, that TURN IP is not reachable; again, manager↔host does not rely on TURN.

## Gaps / need evidence
- Manager logs around 13:58:35–14:00Z for these session_ids to see controller ack/ready state, any errors, and why fallback was allowed.
- Browser doesn’t log pc.onconnectionstatechange/oniceconnectionstatechange/datachannel close; we still don’t know why the browser peers disconnect at ~14:00.
- No explicit reason emitted for manager’s “allowing HTTP fallback” in agent log.

## What to log next run (to guarantee root cause)
1) Manager controller pipeline (session_ids above):
   - When controller transport is marked ready/not-ready; why HTTP fallback is allowed (include reason string).
   - Action enqueue/dequeue with transport used and whether ACKs were observed.
   - Any errors on viewer data channels (close/error events with code/reason).
2) Browser (viewer/connector):
   - pc.onconnectionstatechange/oniceconnectionstatechange and ondatachannel close/error with peer_id + session_id; log before fallback trigger.
   - Log when connector switches transport with the triggering condition (e.g., ack stall, channel close).
3) Host:
   - Datachannel close/error events for dashboard peers (peer_id, reason, bytes/packets sent/received) before “peer left”.
4) Manager API:
   - Keep the new LoggedJson around layout PUTs; capture any 4xx with body (not hit this run, but helpful).
5) Networking experiments:
   - Run once with VPN/WARP off to remove 172.64.150.54 NAT and see if ACK stalls disappear.
   - Run once with TURN forced (`NEXT_PUBLIC_FORCE_TURN=1` in surfer) and ensure TURN/ICE advertise a public IP reachable from the browser (not just 192.168.68.52) when off-LAN; or temporarily bind TURN to 127.0.0.1 if browser is on the same host.

## Interim read
- Manager↔host RTC healthy.
- Browser↔host RTC established but controller actions stalled; browser fell back to HTTP and later closed peers. Root cause is an ACK/ready/forwarding failure, not ICE failure. Further logging above should pinpoint whether the stall is in manager forwarding or browser datachannel closure.

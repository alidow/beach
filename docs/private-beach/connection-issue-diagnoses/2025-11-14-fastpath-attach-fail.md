# Fast-Path Collapse With No HTTP Fallback (Nov 14, 2025)

## Context
- Showcase run with LHS/RHS Python paddles and the mock agent.
- Relevant logs: `temp/pong.log`, `~/beach-debug/beach-host-{lhs,rhs,agent}.log`, `temp/beach-manager.latest.log`, `temp/webrtc_internals_dump`.

## Observations
1. **Manager handshakes and controller forwarders do start.**  
   - For sessions such as `67f1717e-e366-42ff-a34f-e0272b1084b0`, Beach Manager logs `manager handshake dispatched` followed by `controller forwarder connected transport="fast_path"` (see `logs/beach-manager/beach-manager.log:73402647-73431703`).

2. **Manager-side WebRTC repeatedly fails ICE.**  
   - At `17:22:15Z`, `logs/beach-manager/beach-manager.log:74654538-74654572` shows a wave of `ice connection state … new_state=Failed` for that session and its peers.  
   - The `temp/webrtc_internals_dump` contains only `addIceCandidate` entries for the host-side (192.168.68.51 / 72.227.131.114) and no `onicecandidate` events for the manager. The manager answer SDP carries `c=IN IP4 0.0.0.0`, so the container never advertises a reachable candidate. Once connectivity checks finish the DTLS transport dies and the controller forwarder cancels.

3. **Hosts repeatedly begin auto-attach but the channel collapses before it stabilizes.**  
   - The host logs show multiple `manager handshake control message received` + `auto-attach hint received` entries for session `67f1717e-…`, so the manager is pushing the correct hint.  
   - However, because the fast-path peer connection fails almost immediately, the controller action consumer never makes sustained progress: the host pauses its HTTP poller for fast-path, then loses the transport before any actions/acks flow.

4. **When fast-path dies, the manager queues actions but the hosts drop before they can consume them.**  
   - After ICE failure we log `fast-path session not registered; falling back to HTTP`, but the repeated disconnects mean there is no active controller transport (fast-path or HTTP) to drain Redis. Queue depth grows and the mock agent’s `queue_action` calls time out. Manually exporting PB tokens is not an acceptable workaround; the fix is to keep the handshake/transport healthy so the host can use the credentials it already receives.

## Conclusions
- The showcase cannot progress because *both* controller transports are unavailable:
  1. Fast-path cannot stay established: the manager container isn’t configured with a public ICE address (`BEACH_ICE_PUBLIC_IP` / `BEACH_ICE_PUBLIC_HOST`), so every `mgr-actions` data channel eventually hits `ICE failed`.
  2. HTTP fallback is disabled: the CLI hosts were launched without a manager bearer token (`beach login` / `PB_MANAGER_TOKEN`), so they never start the controller poller and cannot consume Redis-queued actions.

## Recommendations
1. **Fix manager ICE config.** Ensure the Beach Manager container advertises a routable IPv4 (e.g., export `BEACH_ICE_PUBLIC_IP=<host LAN IP>` before starting Docker). That lets the controller forwarder keep the `mgr-actions` channel alive instead of dropping to `ICE failed`.
2. **Let the handshake drive controller access.** The host already receives session-scoped controller tokens via `manager handshake…auto-attach` messages. Keep the WebRTC/attach path stable so the existing handshake can succeed—do **not** attempt to patch over the issue by exporting PB tokens manually.
3. **Re-run showcase once fast-path is stable.** With a working WebRTC transport (and automatic HTTP fallback only when necessary), queue_action requests should stop timing out and the mock agent can drive the paddles.

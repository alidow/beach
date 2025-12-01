# 2025-11-28 WebRTC fallback follow-up (env + hypotheses)

## Fresh env observations (containers)
- `beach-manager env`:
  - `BEACH_GATE_TURN_URLS=turn:192.168.68.52:3478`
  - **`BEACH_TURN_EXTERNAL_IP` is unset** (shows as `None` in connect attempts).
  - `BEACH_ICE_PUBLIC_HOST=192.168.68.52`, `BEACH_ICE_PORT_START/END=62500-62600`.
- `beach-road env`:
  - `BEACH_PUBLIC_SESSION_SERVER=http://192.168.68.52:4132`
  - `BEACH_ICE_PUBLIC_HOST=192.168.68.52`, same port range.
- `beach-coturn env`: minimal; no external IP injected (implies turnserver uses defaults; no `external-ip`).

## Why this matters
- With `BEACH_TURN_EXTERNAL_IP` unset, coturn will not emit valid relay candidates; manager/hosts/browsers only see srflx/“private” candidates. This matches the host logs: browser and manager peers offered srflx and private, **no relay**.
- The browser↔host link in the latest run succeeded, then died ~1 minute later; hairpin-only srflx paths are brittle (NAT binding expiry, asymmetry, or ISP-side filtering).
- The manager↔host link shows open data channels but the controller forwarder later declares `rtc_ready=false` and falls back to HTTP. Without relay, ICE still formed (manager+host on Docker network), so the fallback likely stems from forwarder health/ack logic, not a full disconnect—but the lack of relay could exacerbate packet loss or stalled DTLS keepalives if host–manager path hairpins through host IP instead of Docker bridge.

## Additional “crazy” angles to consider
- **Docker host publish vs. container IP mix:** ICE advertises `192.168.68.52` (host LAN) while manager/road live on 172.28.0.0/16. If iptables NAT/forwarding on the host is stale, srflx traffic could intermittently fail while containers still “think” they’re connected.
- **Double-NAT / WARP / VPN:** Browser srflx came from `172.64.150.54` (Cloudflare). That path may have short-lived bindings; without relay, reconnects are likely. Ensure browser is not behind WARP/VPN or add TURN that is reachable from both sides.
- **TURN realm/secret OK but missing external-ip:** Coturn will accept allocs but emit candidates with its container IP (or none) unless `external-ip` is set. That produces invisible/invalid relay candidates—consistent with logs.
- **Manager forwarder health:** The fallback to HTTP starts when `rtc_ready=false` is logged (multiple “drain_actions_redis … rtc_ready=false”). Could be a missing heartbeat on the controller data channel (keepalive frames blocked? send errors unlogged?). Add logging to capture channel state/buffered_amount when rtc_ready flips.
- **Port range exposure:** Ports 62500–62600 must be published on the host. If Docker host publish is missing or interfered with by another service, srflx traffic might work briefly then die when a new 5‑tuple is picked. Verify host-level `iptables`/`pf` NAT and that the UDP range is actually mapped.

## Updated actions / logging to guarantee root cause next run
1) Set TURN correctly for all services:
   - Export `BEACH_TURN_EXTERNAL_IP=192.168.68.52` (or your true host/LAN IP) before `docker compose up`/`dockerup`.
   - Keep `BEACH_GATE_TURN_URLS=turn:192.168.68.52:3478` (no 127.0.0.1), and ensure the same inside manager/gate/hosts.
2) Collect relay evidence:
   - Log ICE servers list and **selected candidate pair** (type/protocol) on host and manager.
   - In the browser (webrtc-internals), verify relay candidates appear after the TURN fix.
3) Instrument RTC readiness flips in controller forwarder:
   - When `rtc_ready` changes or HTTP fallback is chosen, log peer_id/transport_id, data channel state, last sent/recv timestamp, buffered_amount, and any send errors.
4) Log datachannel close/error events explicitly on both host and manager for manager peers.
5) If fallback persists after TURN fix, capture host/manager tcpdump on 62500–62600 and 3478 to see if UDP stops flowing (host firewall/ISP UDP throttling).

If the TURN fix yields relay candidates and stable RTC, the root cause was missing `BEACH_TURN_EXTERNAL_IP`. If not, the added logging + packet capture will isolate whether the forwarder is mis-detecting health despite an active channel. 

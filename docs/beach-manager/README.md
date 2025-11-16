# Beach Manager WebRTC Notes

This document consolidates the fixes and conventions we rely on to keep Beach
Manager’s WebRTC fast‑path working inside the Docker stack.

## Goals

* Manager containers run on the Docker bridge but must advertise host‑reachable
  candidates so CLI hosts (usually on macOS) can complete ICE.
* Public Beach sessions **never** talk to Beach Gate or TURN. Their hosts rely on
  the embedded config in `apps/beach/src/transport/webrtc/mod.rs` (auto-detected
  LAN IP + STUN list).
* Managed/private Beach sessions can use TURN via Beach Gate, but must keep the
  same host/STUN hints so fast-path is preferred.

## Required Docker configuration

* `docker-compose.yml` publishes `62000-62100/udp` and sets `BEACH_ICE_PORT_START`
  / `BEACH_ICE_PORT_END` so the WebRTC transport binds within that range.
* `.envrc` auto-detects the laptop’s LAN IPv4 and exports
  `BEACH_ICE_PUBLIC_IP`/`BEACH_ICE_PUBLIC_HOST`. Running `direnv allow` once is
  mandatory; every `docker compose` command should be wrapped with
  `direnv exec .` so the hints reach the container.
* `scripts/docker/beach-manager-entry.sh` resolves `BEACH_ICE_PUBLIC_HOST`
  inside the container and exports `BEACH_ICE_PUBLIC_IP` if the host provided
  only a DNS name.

Without these hints the manager advertises its 172.x container IP, and fast-path
falls back to HTTP even when the viewer looks “connected”.

## ICE / STUN / TURN selection

* Manager code (`apps/beach-manager/src/fastpath.rs`) prefers env overrides
  (`BEACH_ICE_SERVERS`). Otherwise it uses the default STUN list:
  `stun:stun.l.google.com:19302` plus the dev coturn at
  `stun:host.docker.internal:3478`. Both require outbound UDP access.
* For managed sessions we call Beach Gate’s `/turn/credentials` when available.
  Environment variables for Gate/coturn are tracked in `.env.local`
  (`BEACH_GATE_TURN_SECRET`, `BEACH_GATE_TURN_URLS`, etc.).
* Public CLI hosts (Pong players, smoke tests, etc.) set
  `BEACH_PUBLIC_MODE=1` and rely on the bundled STUN list from
  `apps/beach/src/transport/webrtc/mod.rs`. They **never** hit Beach Gate or
  coturn; all ICE data is local.

## IPv4-only WebRTC

Docker’s default bridge rarely has IPv6 connectivity, but the Rust webrtc crate
will happily probe `udp6` candidates. Those probes:

* Flood logs with `could not get server reflexive address udp6 …`
* Stall ICE long enough that the manager gives up on the fast-path

Mitigation (already in the codebase):

* `apps/beach-manager/src/fastpath.rs`: `setting.set_network_types(vec![NetworkType::Udp4])`
* `apps/beach/src/transport/webrtc/mod.rs`: both offerer and answerer paths call
  `SettingEngine::default(); … set_network_types(vec![NetworkType::Udp4])`

Leave a comment if you touch these sections—IPv6 candidates inside Docker are
still unusable, so reverting this breaks fast-path immediately.

## Issues we have already hit

| Symptom | Root cause | Fix |
| --- | --- | --- |
| `docker compose up` complains about missing `BEACH_ICE_PUBLIC_*` | direnv not loaded | Run `direnv allow`, wrap compose commands with `direnv exec .` |
| Viewer tiles stay yellow, manager logs `fast-path state channel not established` | Manager advertised container IP / no srflx candidates | Ensure `BEACH_ICE_PUBLIC_IP` is set _and_ STUN is reachable |
| Manager logs `could not get server reflexive address udp6 … io error 101` | IPv6 STUN probes timing out | Force IPv4 in SettingEngine (already done) |
| `transport setup failed: timeout waiting for data channel` after many attempts | Docker host blocking outbound UDP (VPN/firewall) | Test UDP connectivity (see below) and adjust host/VPN |
| Public hosts trying to fetch TURN / Beach Gate | `BEACH_PUBLIC_MODE` unset | Keep env snippets from `docs/helpful-commands/pong.txt`; public hosts must remain STUN-only |

## Troubleshooting checklist

1. **Verify env hints**: `direnv status` should show `BEACH_ICE_PUBLIC_IP`. Inside
   `beach-manager`, check `echo $BEACH_ICE_PUBLIC_IP`.
2. **Confirm UDP reachability** (from inside the container):

   ```bash
   python3 - <<'PY'
   import os, socket
   request = b'\x00\x01\x00\x00\x21\x12\xA4\x42' + os.urandom(12)
   sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
   sock.settimeout(3)
   sock.sendto(request, ('74.125.250.129', 19302))
   try:
       data, _ = sock.recvfrom(64)
       print("STUN reply", len(data))
   except Exception as e:
       print("STUN failed", e)
   PY
   ```

   Failure means the host firewall/VPN is blocking container UDP.

3. **Watch manager logs**:

   * `direnv exec . docker compose logs beach-manager | rg fast-path`
   * Look for `fast-path data channel ready` soon after hosts connect.

4. **Public hosts** should show `BEACH_PUBLIC_MODE=1` and the STUN list in their
   `beach-host-*.log` (`using ICE servers from BEACH_ICE_SERVERS server_count=1`).

5. **Managed hosts / mock agent** must either:

   * Run with TURN properly configured in `.env.local`, or
   * Switch to public mode with explicit `BEACH_ICE_PUBLIC_IP`.

## Where things live

* Docker entry: `scripts/docker/beach-manager-entry.sh`
* Fast-path transport: `apps/beach-manager/src/fastpath.rs`
* CLI WebRTC transport: `apps/beach/src/transport/webrtc/mod.rs`
* Dev coturn config: `config/coturn/dev-turnserver.conf`
* Helpful env snippets: `docs/helpful-commands/pong.txt`

Keep this README in sync when you touch any of the above. Most fast-path outages
we’ve debugged boil down to either missing NAT hints or STUN reachability from
the container—document any new gotchas here so we don’t repeat the incident.

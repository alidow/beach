# testing/webrtc-tester plan

## Purpose
Create a self-contained tester under `testing/webrtc-tester/` that proves (or disproves) stable WebRTC connectivity between a beach host running inside a dedicated Docker network and two clients: one inside the same network and one outside (host machine). Use the existing `apps/beach` binary as both host and client to keep parity with production code. Provide smoke tests that send/receive frames with checksums and fail fast on connectivity/health issues.

## Directory layout
```
testing/webrtc-tester/
  docker-compose.yml        # dedicated network/ports, no overlap with main stack
  README.md                 # how to run, scenarios, interpreting results
  scripts/
    start.sh                # bring up compose
    stop.sh                 # bring down compose
    smoke-internal.sh       # inside-client → host test
    smoke-external.sh       # host (outside) → host test
  env.example               # template for IPs/ports/env knobs
  client/                   # wrapper to run apps/beach in client mode (reuse binary)
  host/                     # wrapper to run apps/beach in host mode
```

## Networking/ports (avoid main stack collisions)
- Use a dedicated network name (e.g., `webrtc-tester-net`).
- Assign unique ports:
  - Signaling/API: 5232 (host exposed), internal 5232.
  - Optional TURN: 5233 UDP/TCP (only if we include coturn for diagnostics).
  - Avoid 4132/4133/8080 to allow parallel runs with main stack.

## Components
1) **Host (inside compose)**
   - Run `apps/beach` in host mode with minimal config:
     - Signaling endpoint (HTTP/WS) bound to 0.0.0.0:5232.
     - ICE hints configurable via env: `BEACH_ICE_PUBLIC_IP`, `BEACH_ICE_PUBLIC_HOST`, `BEACH_ICE_PORT_START/END`.
     - Optional TURN config if we include coturn (for browser-like diagnostics).
   - Expose logs to volume for inspection.
   - Provide a simple “echo” behavior on a datachannel: whatever the client sends, host echoes with a checksum.

2) **Clients**
   - **Internal client (inside compose):** Run `apps/beach` in a lightweight client mode (use existing CLI if available) that:
     - Connects to signaling at `http://host:5232` (service name).
     - Establishes WebRTC, opens a datachannel, sends a test frame with checksum, awaits echo, verifies checksum.
   - **External client (host machine):** A small wrapper script that runs `apps/beach` in the same client mode, pointing at `localhost:5232` (port mapped from container). Same checksum exchange and verification.
   - Both clients should log ICE servers received, selected candidate pair (type/proto/IP/port), and datachannel open/close.

3) **Optional TURN (for diagnostics)**
   - Include a coturn service in compose on port 5233 with `EXTERNAL_IP` configurable.
   - Allow toggling TURN use/relay-only via env; default off for host↔manager testing (not needed inside Docker).

## Smoke tests
- **Internal smoke:** `scripts/smoke-internal.sh`
  - Calls `./scripts/start.sh` (if not already running).
  - Runs internal client once; asserts:
    - WebRTC connects.
    - Datachannel opens.
    - Checksum round-trip matches within timeout.
    - Selected candidate pair logged.
  - Returns non-zero on failure; prints logs locations.

- **External smoke:** `scripts/smoke-external.sh`
  - Assumes compose up.
  - Runs external client on host, pointing to `localhost:5232`.
  - Same assertions as internal.

Both smokes should be safe to run in parallel with main stack (distinct ports/network).

## Implementation steps (incremental)
1) Scaffold `testing/webrtc-tester/` with README, env.example, scripts stubs.
2) Write `docker-compose.yml`:
   - Services: host (apps/beach in host mode), optional coturn, internal client (one-shot job service).
   - Network: `webrtc-tester-net`.
   - Ports: map 5232:5232 for signaling; map 5233:5233 UDP/TCP if coturn included.
3) Host wrapper:
   - Docker command to run `apps/beach` host with env from `.env` or compose (ICE hints, port range 62550-62650 to avoid main range).
   - Echo datachannel behavior (could be a mode/flag in beach or a small shim that pipes frames back).
4) Client wrapper (reuse apps/beach):
   - Mode that takes signaling URL and performs WebRTC attach + datachannel send/echo verify.
   - Outputs JSON summary (connected, candidate pair, checksum ok).
5) Scripts:
   - `start.sh`: `docker compose up -d` in this dir.
   - `stop.sh`: `docker compose down -v`.
   - `smoke-internal.sh`: `docker compose run --rm internal-client`.
   - `smoke-external.sh`: runs host-side client binary against `localhost:5232`.
6) README:
   - Purpose, prerequisites, how to run smokes, how to toggle TURN, how to inspect logs/results.
   - Example outputs and troubleshooting (e.g., if external fails, suggest setting `BEACH_ICE_PUBLIC_IP` to host LAN IP).

## Validation and outputs
- Each client run writes a JSON summary (pass/fail, selected candidate, RTT, checksum result) to `testing/webrtc-tester/results/` (or container volume) and logs to stdout.
- Smoke scripts exit non-zero on failure so they can be CI’d later.

## Notes
- TURN is optional and primarily for browser-like diagnostics; host↔host inside Docker should connect without it.
- Keep port ranges and network isolated to allow concurrent main stack usage.
- Reuse existing beach binary to stay close to production code paths; only add minimal wrappers.

# WebRTC Tester (apps/beach)

This harness brings up a minimal Beach session server + host inside an isolated Docker network and exercises WebRTC from two angles:

- **Host**: `apps/beach` running in host mode with a tiny echo shell on the terminal data channel.
- **Internal client**: runs inside the compose network against `http://signaling:5232`.
- **External client**: runs on the host machine against `http://localhost:5232`.

The goal is to validate signaling + ICE on unique ports (5232 for signaling, 62550–62650 for ICE) without clashing with the main stack.

## Prerequisites
- Docker + docker compose installed locally.
- Rust toolchain available on the host (for the external smoke client).
- Python 3 available in your shell (container entrypoints install it if missing).

## Layout
- `docker-compose.yml` — signaling (beach-road), host, internal client, optional TURN.
- `env.example` — copy to `.env` to override ICE hints/port ranges.
- `scripts/start.sh` / `scripts/stop.sh` — bring the stack up/down.
- `scripts/smoke-internal.sh` — runs the internal client via `docker compose run`.
- `scripts/smoke-external.sh` — runs the host-side client against `localhost:5232`.
- `host/` — host entrypoint + echo shell (returns sha256 of input).
- `client/` — Python harness that drives the `apps/beach` CLI through a PTY and checks echo/candidate logs.
- `results/` — JSON summaries and raw logs.

## Quick start
```bash
cd testing/webrtc-tester
cp env.example .env   # tweak BEACH_ICE_PUBLIC_IP/HOST if needed
./scripts/start.sh
```

Stop everything:
```bash
./scripts/stop.sh
```

## Smoke tests
- Internal (inside compose): `./scripts/smoke-internal.sh`
- External (host machine): `./scripts/smoke-external.sh`

Each smoke writes a summary JSON under `testing/webrtc-tester/results/` with status, checksum result, and the first candidate log line found in the client log.

To hold a client open for stability checks, set `CLIENT_HOLD_SECS` (e.g., `CLIENT_HOLD_SECS=90 ./scripts/smoke-external.sh`) and optionally bump `CLIENT_TIMEOUT`.

## TURN toggle
- TURN is off by default. Enable with compose profiles: `direnv exec . docker compose --profile turn up -d`.
- Set `TURN_EXTERNAL_IP` and `BEACH_ICE_SERVERS` in `.env` (example: `[ { "urls": ["turn:${TURN_EXTERNAL_IP}:5233?transport=udp"], "username": "${TURN_USERNAME}", "credential": "${TURN_PASSWORD}" } ]`).
- The UDP port range defaults to 62550–62650; adjust `TURN_MIN_PORT`/`TURN_MAX_PORT` if you change it.

## Logs and results
- Host handshake: `testing/webrtc-tester/results/host-handshake.json`
- Host logs: `testing/webrtc-tester/results/host-stdout.log`, `.../host.log`
- Client captures: `.../internal-client-capture.log`, `.../external-client-capture.log`
- Summaries: `...-summary.json` (exit non-zero on failure)

## Notes
- Ports intentionally differ from the main stack (avoid 4132/4133/8080).
- ICE port range defaults to 62550–62650; keep it distinct if you run the main stack in parallel.
- The echo shell prints `ECHO_SERVER_READY`, then echoes any line with a sha256. The clients verify the checksum and log the first candidate line from the Beach logs.

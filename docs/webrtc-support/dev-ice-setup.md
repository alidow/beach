WebRTC Fast-Path Dev Setup
==========================

This note distills the "public vs managed" ICE plan into actionable steps for laptop
development. Follow these recipes whenever you need deterministic fast-path WebRTC across
the two supported topologies:

- **Manager in Docker, hosts on macOS** — the default `docker compose up beach-manager` stack.
- **Everything on the host** — running manager/road/gate/coturn directly on the laptop.

Both scenarios use the same environment variables and smoke tests, so once you have the
values below in place the WebRTC behavior is identical across harness apps (Pong, Beach
Surfer, etc.).

Manager / Docker configuration
------------------------------

1. Ensure the manager container publishes a UDP range (already wired in `docker-compose.yml`).
   The defaults expose `62000-62100/udp`.
2. Keep `docker compose`’s `extra_hosts` entry for `host.docker.internal` so containers can
   resolve the host gateway.
3. The manager always talks to STUN (Google + the dev coturn on `host.docker.internal:3478`) so it
   advertises a reflexive/relay address automatically. Laptop devs no longer need to export
   `BEACH_ICE_PUBLIC_IP`; those overrides are only necessary when you want to pin a specific
   NAT mapping (e.g., a remote VM).

Local CLI / host configuration
------------------------------

### Public-mode (no Beach Gate, STUN only)

Use this configuration for the widely distributed binary and for quick local testing when
you do not want to rely on Beach Gate or TURN:

```bash
export BEACH_PUBLIC_MODE=1
export BEACH_ICE_SERVERS='[{"urls":["stun:host.docker.internal:3478","stun:127.0.0.1:3478"]}]'
```

- The CLI never talks to Beach Gate or coturn.
- WEBRTC gathers host/prflx candidates plus the explicit STUN servers you provide.
- Perfect for Pong, smoke tests, or any harness running on the same laptop as the manager.

### Managed-mode (logged in, TURN available)

When you run Private Beach scenarios and need TURN as a fallback:

```bash
export BEACH_PUBLIC_MODE=0   # or simply unset BEACH_PUBLIC_MODE
beach auth login             # ensure you have a valid profile
```

Leave `BEACH_ICE_SERVERS` unset so the CLI fetches the TURN bundle from Beach Gate. The
fast-path remains the primary controller channel; coturn is only a fallback when a public
candidate pair cannot be negotiated.

Smoke tests
-----------

`scripts/fastpath-smoke.sh` exercises both paths end-to-end:

```bash
# Run both public (STUN-only) and managed (TURN-capable) modes
FASTPATH_SMOKE_MODES=public,managed scripts/fastpath-smoke.sh
```

- `public` mode exports `BEACH_PUBLIC_MODE=1` and the STUN list above, then verifies that
  no "using TURN credentials from Beach Gate" log entry appears.
- `managed` mode ensures you are logged in (via `FASTPATH_SMOKE_GATE_PROFILE`) and asserts
  that TURN credentials were fetched successfully.
- Override `FASTPATH_SMOKE_LOCAL_ICE` if you need a different STUN bundle for laptops on
  other networks.

Pong / harness workflows
------------------------

Refer to `docs/helpful-commands/pong.txt` for the canonical cargo commands. Before launching
hosts or agents:

1. Export the STUN + public-mode vars shown above when you want to stay in the public/STUN
   configuration.
2. Drop `BEACH_PUBLIC_MODE` and `BEACH_ICE_SERVERS` (or set `BEACH_PUBLIC_MODE=0`) and log
   in when you want to validate the managed/TURN behavior.

With these few environment variables in place the CLI behaves identically between the
smoke scripts, Pong, and any Beach harness: the fast-path WebRTC channels come up quickly
on localhost, and TURN/coturn traffic only occurs when you explicitly opt into the managed
flow.

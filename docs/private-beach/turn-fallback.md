# Private Beach TURN Fallback

## Purpose
- Keep WebRTC sessions alive for paid private beach customers when direct peer-to-peer negotiation fails.
- Require Beach login + Beach Gate authorization so the shared TURN pool cannot be abused by anonymous traffic.
- Support tiered offerings (`private-beach:turn`, `private-beach:turn:pro`, ...) without reconfiguring coturn.

## Flow
- Authenticated clients call `POST /turn/credentials` on Beach Gate with an access token.
- Beach Gate verifies entitlements and signs a short-lived TURN credential (HMAC-SHA1, default TTL 120s).
- Response payload (`iceServers`) is passed straight into the WebRTC configuration; coturn validates the HMAC via `use-auth-secret`.
- Credentials expire quickly so a leaked username/password pair is useless after ~2 minutes.

## Client Integration
- Beach CLI automatically requests TURN credentials before initiating WebRTC and appends the default public STUN server when enabled, so srflx candidates are still gathered.
- Additional clients should mirror the same flow: present their Beach access token, honor the TTL, and refresh credentials only when negotiating (not on every ICE retry).

## Entitlements & Pricing
- **Base entitlement**: `private-beach:turn` &mdash; included with the entry private beach tier. Grants default bitrate quota.
- **Higher tiers**: extend with suffixes such as `private-beach:turn:plus` (larger quotas) and `private-beach:turn:priority` (reserved capacity). A user qualifies if they hold *any* entitlement in `BEACH_GATE_TURN_REQUIRED_ENTITLEMENTS`.
- **Billing seed**: private beach billing sync should write the appropriate entitlement string for every active subscription and remove it immediately on downgrade or cancellation. Store tier metadata alongside the subscription so the entitlement map stays authoritative.

## Configuration (Beach Gate)
- Environment variables:
  - `BEACH_GATE_TURN_SECRET`: shared static auth secret (must match coturn).
  - `BEACH_GATE_TURN_URLS`: comma-delimited TURN URLs exposed to clients (e.g. `turns:turn.private-beach.sh:5349,turn:turn.private-beach.sh:3478`).
  - `BEACH_GATE_TURN_REALM`: realm advertised to clients (defaults to `turn.beach.sh`).
  - `BEACH_GATE_TURN_TTL`: credential lifetime in seconds (default 120).
  - `BEACH_GATE_TURN_REQUIRED_ENTITLEMENTS`: comma list of entitlements that unlock TURN (defaults to `private-beach:turn`).
- Access tokens lacking any of the required entitlements receive HTTP 403; if TURN is not configured, Beach Gate responds with HTTP 503.
- Username format: `${expiryEpoch}:${sub}:${tier}` so coturn logs still reveal the user and their billing tier.

## Dev Docker Setup
- `docker-compose up coturn` launches a local coturn instance with config from `config/coturn/dev-turnserver.conf`.
- The dev config disables TLS and DTLS, publishes ports `3478/tcp+udp` and `5349/tcp+udp`, and uses the static secret `beach-dev-turn-secret`.
- Set matching Beach Gate env vars before running the service locally:
  ```bash
  export BEACH_GATE_TURN_SECRET=beach-dev-turn-secret
  export BEACH_GATE_TURN_URLS=turn:127.0.0.1:3478
  export BEACH_GATE_TURN_REALM=turn.private-beach.test
  export BEACH_GATE_TURN_REQUIRED_ENTITLEMENTS=private-beach:turn
  ```
- When Beach Gate runs inside Docker, point URLs at the coturn service name instead: `turn:beach-coturn:3478`.
- Request credentials manually for smoke tests:
  ```bash
  curl -sS -X POST http://localhost:4133/turn/credentials \
    -H "Authorization: Bearer <access-token>" | jq
  ```
- Rotate the dev secret by updating both the config file and the env var, then `docker compose restart coturn`.

## Production Checklist
- Run coturn behind an internal load balancer with TLS certificates and health checks.
- Keep two active auth secrets during rotation; update Beach Gate, wait out TTL + a grace period, then drop the old secret.
- Configure quotas (`total-quota`, `bps-capacity`) per tier and feed metrics into the existing observability stack.
- Alert on exhausted quotas or spikes in rejected allocations so capacity can be tuned ahead of customer impact.

## Plan: Gate-issued TURN creds with TTL and client refresh

### Goal
Provide the browser with time-bound TURN credentials (default 4 hours) issued by Beach Gate so WebRTC can use a stable relay without shipping static secrets. Manager fetches creds per join, browser refreshes before expiry.

### Current state
- Browser join path: `GET /sessions/:id/join` (see `connectBrowserTransport` in `apps/beach-surfer/src/terminal/connect.ts`). Manager returns signaling info; currently no TURN creds surface to the browser; it falls back to default Google STUN.
- Gate has TURN config (`BEACH_GATE_TURN_URLS/SECRET/REALM/TTL`) but manager doesn’t plumb `ice_servers` through.
- UI env can set `NEXT_PUBLIC_TURN_*`, but that injects static TURN, not Gate-issued.

### Desired flow
1) Browser hits manager: `GET /sessions/:id/join`.
2) Manager calls Gate for TURN creds (using requester auth): Gate returns long-term TURN entries with `urls/username/credential` and an expiry derived from TTL (e.g., 4h).
3) Manager includes `ice_servers` and `expires_at_ms` (when creds expire) in the join response.
4) Browser uses those `ice_servers` to build RTCPeerConnection; no static TURN in UI env needed.
5) Browser refreshes creds before expiry: re-run join to get fresh `ice_servers`, then reconnect/ICE restart.

### TURN TTL mechanics
- Coturn supports time-bound long-term credentials: username encodes expiry (e.g., `<unix_expiry>:<id>`); credential is HMAC(secret, `username:realm`). Coturn validates HMAC and expiry on allocate.
- Gate should mint usernames with expiry = now + TTL (default 4h) using its shared secret/realm and `BEACH_GATE_TURN_URLS`.
- Gate config: set `BEACH_GATE_TURN_TTL=14400` (seconds) for 4h. `BEACH_GATE_TURN_URLS`, `BEACH_GATE_TURN_SECRET`, `BEACH_GATE_TURN_REALM` already present.

### Manager changes
- Add a helper to fetch TURN creds from Gate (use existing auth client if present) and return an `ice_servers` array plus `expires_at_ms`.
- In the join handler, after auth, call Gate for TURN, merge with any STUN if needed, and attach `ice_servers` + `ice_servers_expires_at_ms` to the join payload consumed by the browser.
- Ensure the manager’s env has Gate URLs/secret/realm to call the TURN creds endpoint (if not already present).

### Browser changes
- In `connectBrowserTransport` (`apps/beach-surfer/src/terminal/connect.ts`), prefer `join.ice_servers` if present; only fall back to env when missing. The helper `maybeParseIceServers` already exists; wire it as fallback, not primary.
- Handle `ice_servers_expires_at_ms`: schedule a refresh before expiry (e.g., at 80% TTL). Refresh means re-running join to get fresh `ice_servers` and reconnecting (reuse existing reconnect flow by tearing down and reconnecting with new servers).
- Keep the default STUN only when no TURN is provided; do not pass empty TURN creds to RTCPeerConnection (guard already present).

### Client refresh logic
- When join response includes `ice_servers_expires_at_ms`, set a timer for `expires_at_ms - now - safety`, where safety ~20% of TTL or fixed (e.g., 10 minutes). On timer, re-run join to fetch fresh `ice_servers`, then reconnect/ICE restart.
- If the transport closes earlier, reconnect flow naturally fetches fresh join data and uses the new `ice_servers`.

### Validation steps
- Verify Gate returns TURN creds with expiry when called manually (curl manager → Gate or direct Gate endpoint).
- After wiring manager, hit `GET /sessions/:id/join` and confirm payload contains `ice_servers` entries with `urls/username/credential` and `ice_servers_expires_at_ms`.
- In the browser, open `chrome://webrtc-internals` and confirm `rtcConfiguration.iceServers` lists the Gate-issued TURN and selected candidates include `relay`.
- Let a session run past the prior 60–90s drop; ensure it stays connected and/or refreshes creds before TTL.

### Env/config to set
- Gate: `BEACH_GATE_TURN_URLS` (publicly reachable TURN), `BEACH_GATE_TURN_SECRET`, `BEACH_GATE_TURN_REALM`, `BEACH_GATE_TURN_TTL=14400`.
- Manager: ensure it knows how to call Gate (issuer/audience/secret) to fetch TURN creds and expose them in join.
- UI: no static TURN env needed once join payload supplies `ice_servers`.

### Files likely to touch
- Manager join handler: include `ice_servers` in `GET /sessions/:id/join` response (check apps/beach-manager for join endpoint) and Gate client to fetch TURN creds.
- `apps/beach-surfer/src/terminal/connect.ts`: consume `join.ice_servers`, fallback to env only when absent; pass to `connectWebRtcTransport`.
- `apps/private-beach/src/hooks/sessionTerminalManager.ts`: ensure join response is used as-is; no hardcoded ICE servers.
- Optional: shared type for join payload to include `ice_servers` and `ice_servers_expires_at_ms`.

### Notes/risks
- Ensure Gate TURN creds are per-requester and time-bound; do not cache indefinitely in manager.
- Avoid passing TURN entries with empty username/password to RTCPeerConnection (guard already in place).
- Coturn must be reachable on the advertised IP/port (`--external-ip` set to host/public IP) so relay candidates are usable.

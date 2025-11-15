# 2025-11-14 – JWKS mismatch killed controller traffic

## What we observed
- During the latest local "pong" showcase run every tile reported `fast-path:success` (temp/pong.log:730-1757) and Chrome's `webrtc_internals_dump` shows `iceconnectionstatechange` reaching `"connected"` for the three peer connections, so WebRTC transport and data channels were alive.
- Around 12:20 PM local (`17:20Z`) the manager log (`temp/manager-tail.log:435+`) started spamming `token verification failed UnknownKey("796dfeda1e496953")` for every privileged dashboard request: controller lease polling, `/state`, `/layout`, etc. No controller lease ever renewed, so the agent tile never got paired despite the transport being up.
- The failing `kid` (`796dfeda1e496953`) matches the only key beach-gate currently serves (`curl http://localhost:4133/.well-known/jwks.json`). That means beach-gate restarted/rebuilt (rolling its signing key), but beach-manager never reloaded the JWKS and therefore treats every token as untrusted.
- About a minute later (`temp/manager-tail.log:780-810`) the manager tears down all WebRTC peers with `[controlled]: Setting new connection state: Disconnected` + ICE `disconnected` events because it stopped receiving controller heartbeats.

## Root cause
Beach-gate minted a new signing key without beach-manager being restarted. Manager caches the JWKS it fetched on boot, so after the gate reboot all dashboard tokens carry an unknown `kid` and every controller/API call fails with HTTP 401. Fast-path stays connected, but controller pairing never completes, so the game cannot progress.

## Fix / mitigation
1. Restart beach-manager whenever beach-gate restarts so the JWKS cache is refreshed (or add periodic JWKS refresh logic; today it only loads on boot).
2. For quicker local testing you can temporarily set `AUTH_BYPASS=1` in `.env`/`docker-compose` to skip token verification, but the real fix is to keep gate's signing keys stable (persist the PEM) or teach manager to re-fetch JWKS when it encounters an unknown `kid`.
3. After restarting manager, confirm the `UnknownKey` warnings disappear and that controller leases show up again in `temp/manager-tail.log` before re-running the showcase.

## Verification steps
- `docker restart beach-manager` (or bring the compose stack back up) after any beach-gate rebuild.
- Hit `http://localhost:8080/health` to ensure the manager is ready, then redo the showcase. While attaching, tail `docker logs beach-manager` and ensure there are no `token verification failed` warnings.
- From the dashboard, verify tiles reach `fast-path:success` and that the controller arrows appear (controller lease confirmed in manager logs).

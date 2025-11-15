# Pong Showcase: Manager Auth Still Broken (Nov 14 2025)

## Summary

Even after adding fallback pollers on the Python agent, the showcase still refuses to play because the Beach Manager continues to reject every authenticated stream. The controller/agent code never receives pairing events, so transport readiness never flips, and the CLI hosts never finish auto‑attaching. The root cause is the same JWKS mismatch we identified earlier.

## Evidence

1. **Manager rejects Gate tokens** – `temp/beach-manager.latest.log` is still full of `token verification failed error=UnknownKey("5a5f5f194aafe4db")` for every `/sessions/.../state/stream` and `/sessions/.../controllers/stream` call. Those requests come from the CLI hosts and the Python agent. Once they fail, the hosts fall back to “waiting for players“ and manager fast-path never comes online.
2. **Fallback poller never activates** – Because the `/controllers` SSE stream never authenticates, `handle_pairing_event` in `apps/private-beach/demo/pong/agent/main.py` is never invoked. You can verify by grepping `~/beach-debug/beach-host-agent.log`: there are zero `"poller started"` lines even though we log them explicitly.
3. **Hosts stay disconnected at the manager** – Docker logs (`docker compose logs beach-manager`) are a wall of `[controlled]: Setting new connection state: Disconnected` entries. The browser WebRTC dump (`temp/webrtc_internals_dump`) shows the viewer path does reach `connectionstatechange: "connected"`, so the ICE failure is confined to manager <-> host transports.

## Root Cause

`BEACH_GATE_JWKS_URL` is still pointing at the Clerk JWKS (`https://shining-sunbird-15.clerk.accounts.dev/...`) via the repo `.env`, so this manager instance never downloads the Beach Gate keys. Every Gate-minted ES256 token (kid `5a5f5f194aafe4db`) is unknown, which blocks:

- Host `/state/stream` & `/controllers/stream`
- Agent `/controllers/stream`
- Fast-path watchers / attach handshakes

## Fix

1. Update the manager env to use Gate’s JWKS inside docker: `BEACH_GATE_JWKS_URL=http://beach-gate:4133/.well-known/jwks.json`, `BEACH_GATE_ISSUER=beach-gate`, `BEACH_GATE_AUDIENCE=private-beach-manager`. Remove the Clerk override from `.env` before `docker compose up`.
2. Restart beach-manager and validate no `UnknownKey` warnings appear at startup; `rg UnknownKey temp/beach-manager.latest.log` should return nothing.
3. Re-run the showcase. Once the manager accepts Gate tokens, the fallback pollers + controller tokens will finally make the agent/hosts ready, fast-path can establish, and Pong can run again.

Until that env is fixed, any additional agent-side tweaks (pollers, logging, etc.) have no effect because the underlying SSE requests never authenticate.

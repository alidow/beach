# Private Beach Sandbox Playwright Handoff

_Last updated: 2025-03-31_

## Critical Setup

1. **Run Private Beach dev server on a non-default port**
   - Start a dedicated instance for Playwright so local dev at `http://localhost:3001/beaches` stays unaffected.
   - Example: `npm --prefix apps/private-beach run dev -- --hostname 127.0.0.1 --port 3100`
   - Update Playwright config or CLI `baseURL` to match (`http://127.0.0.1:3100`).
2. Ensure no other process is using the chosen port before launching (e.g., `lsof -i :3100`).

## Current Context

- Hook refactor: `useSessionTerminal` now delegates to `sessionTerminalManager.ts`, which keeps a per-key transport/store alive across remounts.
- Fixtures moved under `apps/private-beach/src/sandbox/fixtures-data/` so Next.js can resolve them.
- Sandbox Playwright spec: `apps/private-beach/tests/e2e/private-beach-sandbox.spec.ts`
  - Loads `/dev/private-beach-sandbox?skipApi=1&privateBeachId=sandbox&sessions=sandbox-session|application|Sandbox Fixture&terminalFixtures=sandbox-session:pong-lhs&viewerToken=sandbox-token`.
  - Asserts the “PRIVATE BEACH PONG” banner renders and persists after clicking the tile.
  - Currently uses `baseURL` from config (default `http://localhost:3000`). Adjust when moving to new port.
- Real manager session ID from previous runs was invalidated; generate a fresh one before live testing.

## Pending Work

1. Update Playwright `baseURL` / `webServer` config to match the new dev server port (or remove the auto web server and rely on manual startup).
2. Use the latest test session (schema `2`):
   ```json
   {"schema":2,"session_id":"a6d9faba-85f5-4990-a05f-780ad8e8abf5","join_code":"FDYSQO","session_server":"http://localhost:4132/","active_transport":"WebRTC","transports":["webrtc","websocket"],"preferred_transport":"webrtc","webrtc_offer_role":"offerer","host_binary":"beach","host_version":"0.1.0-20251027174235","timestamp":1761859511,"command":["/usr/bin/env","python3","/Users/arellidow/development/beach/apps/private-beach/demo/pong/player/main.py","--mode","lhs"],"wait_for_peer":true,"mcp_enabled":false}
   ```
   Generate a fresh one if this expires.
3. Extend Playwright coverage to drag/resize once streaming is confirmed.

## Useful Commands

```bash
# Launch dev server on 3100 for Playwright
default_port=3100
npm --prefix apps/private-beach run dev -- --hostname 127.0.0.1 --port $default_port

# Run sandbox spec pointing to the new baseURL
DEBUG=pw:api PW_TEST_HTML_REPORT=0 npx playwright test tests/e2e/private-beach-sandbox.spec.ts \
  --config apps/private-beach/playwright.config.ts \
  --project=chromium \
  --timeout=90000 \
  --workers=1 \
  --retries=0 \
  --reporter=list \
  --base-url "http://127.0.0.1:$default_port"
```

## Logs & Artifacts

- Client logs: `temp/private-beach.log`
- Sandbox Playwright artifacts: `apps/private-beach/test-results/private-beach-sandbox-*/`
- Manager logs: `temp/beach-surfer.log`

## Contact

- Maintain these notes in tandem with `docs/private-beach/tile-connection-issues/README.md`.

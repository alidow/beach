# Pong Fast-Path Test Automation Plan (private-beach-rewrite-2)

Objective: a robust, fast, repeatable smoketest to validate that public host sessions and a private agent session can be attached as tiles to a private beach and all communicate over fast-path (no browser auth errors, no tag mismatches). Target frontend: `private-beach-rewrite-2` at `http://localhost:3003/beaches/<beach-id>`.

## Recommended headless smoketest (no GUI flakiness)
1) **Start stack**  
   Run `direnv exec . ./apps/private-beach/demo/pong/tools/pong-stack.sh start --setup-beach <beach-id>`. Capture the beach id and note `/tmp/pong-stack` paths inside the `beach-manager` container.

2) **Read bootstrap artifacts**  
   From inside beach-manager: `/tmp/pong-stack/bootstrap-lhs.json`, `bootstrap-rhs.json`, `bootstrap-agent.json`. Extract `session_id` and `join_code` (passcode).

3) **Poll readiness (logs only)**  
   - Tail `agent.log` and `beach-host-{lhs|rhs}.log` for readiness markers: “agent ready”, “commands active”, “fast-path ready” (or equivalent).  
   - Fail fast on `authentication tag mismatch`, `fast_path` downgrade, or reconnect loops from `docker logs beach-manager` (search for “fast-path” and “fallback”).

4) **Gameplay assertion**  
   Require at least one ball/score trace in player or agent log (serves, bounces, score increment). Use `grep` with a timeout to detect.

5) **Bound runtime and cleanup**  
   Limit to ~60s total. On success/failure, stop the stack (`pong-stack.sh stop`).

6) **Exit codes**  
   Non-zero on missing readiness markers, tag mismatch, no gameplay events, or fast-path errors.

This path is fast and deterministic; no browser needed.

## Optional UI validation (Playwright or MCP Chrome)
Use a separate “ui smoke” mode:
1) Navigate to `http://localhost:3003/beaches/<beach-id>` (rewrite-2).
2) Wait for three tiles (lhs/rhs app, agent) to appear. If they’re not pre-placed, fail and exit (indicates setup failure).
3) Verify each tile shows connected state (no reconnect spinner), no fast-path errors in console.
4) Optionally open agent tile to see “commands active”/“fast-path ready”. Skip visual gameplay validation; rely on logs for motion/score.
5) Keep timeouts short (30–45s).

### Handling authentication for rewrite-2
- Rewrite-2 typically uses Clerk/Beach auth. For local/dev runs, authentication is usually already satisfied by the surfer container or mocked user (e.g., `mock-user`). If a login is required:
  - Use the existing mock-user flow (check the surfer console; if redirected to Clerk, fill the mock credentials if present).
  - If the surfer exposes a “mock user” button or auto-login, trigger it once at the start of the Playwright script.
  - Preserve session cookies between runs if needed (Playwright storage state).
- If backend expects a manager token, the UI fetches it automatically; ensure `BEACH_GATE`/Clerk env vars are set via `direnv` as per repo guidance.

## Integration into `scripts/pong-fastpath-smoke.sh`
- Add a `--headless` mode:
  - Start stack (`--setup-beach`).
  - Read bootstrap JSON via `docker exec beach-manager cat /tmp/pong-stack/bootstrap-*.json`.
  - Tail `agent.log` and `beach-host-{lhs|rhs}.log` for readiness and gameplay markers.
  - Scan `docker logs beach-manager` for fast-path errors/tag mismatches.
  - Enforce timeouts and exit codes.
- Optional `--ui-smoke` mode: run Playwright against rewrite-2 URL, assert 3 tiles connected, and no fast-path errors in browser console.

## Signals to watch
- Success: readiness markers present; no fast-path errors; at least one ball/score event in logs.
- Failure: `authentication tag mismatch`, `fast_path` fallback/downgrade, missing readiness markers, or no gameplay events within timeout.

## Artifacts (optional)
- Save log snippets for readiness/gameplay.
- UI mode: capture a screenshot of the canvas with three connected tiles.

## Playwright UI check (rewrite-2)
- Test file: `apps/private-beach/tests/e2e/pong-fastpath-live.pw.spec.ts`.
- Env:
  - `PRIVATE_BEACH_ID` (required): beach id used by the current stack run.
  - `PRIVATE_BEACH_URL` (optional, default `http://localhost:3003`).
  - `PONG_BOOTSTRAP_DIR` (optional): host path with `bootstrap-{lhs,rhs,agent}.json` copied from `/tmp/pong-stack` to assert expected session ids; default `temp/pong-fastpath-smoke/latest`.
- Run (after stack is up and layout seeded):
  ```
  npx playwright test tests/e2e/pong-fastpath-live.pw.spec.ts \
    --config apps/private-beach/playwright.config.ts
  ```
- The test captures rewrite-2 telemetry (`canvas.tile.connect.success`), asserts at least three tiles connect, and fails on any `canvas.tile.connect.failure` or fast-path console errors. It skips entirely if `PRIVATE_BEACH_ID` is not set.

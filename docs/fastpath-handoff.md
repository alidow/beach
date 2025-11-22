# Fast-Path Pong Repro + Debug Handoff

Context: LHS application tile still hangs when connecting (no data channel; browser shows spinner). Beach ID: `e2258c9c-07b6-4f0b-ac40-b5a4a451d377`. Latest sessions from `pong-stack`:

- lhs: `394cbc64-fb5e-4d18-ab00-ee41146d1687`, code `DNBMEK`
- rhs: `003409ed-65a6-4934-8fd9-01f2b6cc9a76`, code `1ERU1K`
- agent: `c6788919-63e8-403e-b775-fa653aaef9d9`, code `ISOVYB`

## Bring the stack up exactly as the user did

```bash
# Trust .envrc once per shell
direnv allow

# Reset DB/redis stacks (keeps volumes except postgres)
direnv exec . ./scripts/dockerdown --postgres-only
direnv exec . docker compose down

# Rebuild manager with trace flags
direnv exec . env BEACH_SESSION_SERVER='http://beach-road:4132' PONG_WATCHDOG_INTERVAL=10.0 docker compose build beach-manager

# Start the stack with tracing and insecure dev tokens
DEV_ALLOW_INSECURE_MANAGER_TOKEN=1 \
DEV_MANAGER_INSECURE_TOKEN=DEV-MANAGER-TOKEN \
PRIVATE_BEACH_MANAGER_TOKEN=DEV-MANAGER-TOKEN \
PRIVATE_BEACH_BYPASS_AUTH=1 \
direnv exec . sh -c 'BEACH_SESSION_SERVER="http://beach-road:4132" PONG_WATCHDOG_INTERVAL=10.0 BEACH_MANAGER_STDOUT_LOG=trace BEACH_MANAGER_FILE_LOG=trace BEACH_MANAGER_TRACE_DEPS=1 docker compose up -d'
```

## Seed the Pong sessions on the target beach

```bash
DEV_ALLOW_INSECURE_MANAGER_TOKEN=1 \
DEV_MANAGER_INSECURE_TOKEN=DEV-MANAGER-TOKEN \
PRIVATE_BEACH_MANAGER_TOKEN=DEV-MANAGER-TOKEN \
NEXT_PUBLIC_PRIVATE_BEACH_MANAGER_TOKEN=DEV-MANAGER-TOKEN \
PRIVATE_BEACH_BYPASS_AUTH=0 \
BEACH_SESSION_SERVER=http://beach-road:4132 \
PONG_WATCHDOG_INTERVAL=10.0 \
direnv exec . ./apps/private-beach/demo/pong/tools/pong-stack.sh start e2258c9c-07b6-4f0b-ac40-b5a4a451d377
# Capture bootstrap from the printed table (see sessions above).
```

## Manual UI repro (hang)

1) Open the rewrite UI for the beach: `http://localhost:3003/beaches/e2258c9c-07b6-4f0b-ac40-b5a4a451d377`.
2) Add an Application tile.
3) Enter session ID `394cbc64-fb5e-4d18-ab00-ee41146d1687` and passcode `DNBMEK`, click Connect.
4) Observe: tile hangs attempting to connect (no data channel established within 30s). Browser console/`temp/pong.log` contains the failure trace.

## Logs to grab after a failed connect

```bash
# Browser log (user saved it to temp/pong.log)
tail -n 300 temp/pong.log

# Manager logs (trace enabled)
direnv exec . docker compose logs beach-manager --since=10m | rg "fast_path|webrtc|handshake|secure signaling|candidate|invalid port|state chunk"

# Host player logs inside container
docker exec beach-manager tail -n 200 /tmp/pong-stack/beach-host-lhs.log
docker exec beach-manager tail -n 200 /tmp/pong-stack/beach-host-rhs.log

# Redis offers/answers for the LHS session (helps catch retargeting/AAD mismatches)
docker exec beach-redis redis-cli KEYS '*394cbc64*'
```

## Playwright spec to replicate this exact flow

Add a new e2e test (headful) that:

1) Assumes the stack and beach are already up (commands above).
2) Reads bootstrap files from `PONG_BOOTSTRAP_DIR` (copy `/tmp/pong-stack/bootstrap-*.json` out of the manager container first).
3) Navigates to `http://localhost:3003/beaches/e2258c9c-07b6-4f0b-ac40-b5a4a451d377`.
4) Creates an Application tile, fills session ID and passcode from the LHS bootstrap, clicks Connect.
5) Waits up to 30s for:
   - At least one canvas tile to show status `Connected` (or no error badge),
   - No console errors matching `fast-path|authentication tag mismatch|data channel|webrtc.connect_error`,
   - (Optionally) a manager log scrape that shows `controller forwarder connected … transport="fast_path"` for the LHS session.
6) Fails if the connect spinner persists past 30s or if console logs contain fast-path errors.

Suggested Playwright env:

```bash
SKIP_PLAYWRIGHT_WEBSERVER=1 \
PRIVATE_BEACH_ID=e2258c9c-07b6-4f0b-ac40-b5a4a451d377 \
PRIVATE_BEACH_MANAGER_TOKEN=DEV-MANAGER-TOKEN \
NEXT_PUBLIC_PRIVATE_BEACH_MANAGER_TOKEN=DEV-MANAGER-TOKEN \
PRIVATE_BEACH_BYPASS_AUTH=0 \
PONG_BOOTSTRAP_DIR=$PWD/temp/pong-fastpath-smoke/latest \
PONG_WATCHDOG_INTERVAL=10.0
```

Implementation hints for the test:

- Parse the bootstrap JSONs by slicing between the first `{` and last `}` (they are prefixed with cargo build chatter).
- Use selectors for the Application tile form inputs (session ID, passcode) and the Connect button; assert the tile reaches connected state within 30s.
- Capture and scan `page.on('console')` for fast-path/WebRTC errors during the wait.

## Known prior root cause (for context)

Earlier failures were caused by beach-road retargeting sealed offers to a different `to_peer`, producing AAD/key mismatches (`secure signaling decrypt failed … key_fingerprint=d3aac366f099b0d8`). That logic was removed; retargeting shouldn’t be present now. If you see decrypt or “authentication tag mismatch,” dump the offer payloads from Redis and compare `from_peer`/`to_peer` with the peers the host used when sealing.

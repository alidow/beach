# Fast-path Tag-Mismatch Triage (Pong, rewrite-2)

Context: Manual attach of sessions (e.g., `ddff98de-16c3-4897-ae2c-ef20eeb3dd33` / `LNJ0DH` on beach `66e43c7e-f575-4780-8cff-38a48a7e0cdc`) shows `authentication tag mismatch` in the browser. Manager accepts dev token and `attach_by_code` succeeds, but WebRTC negotiation times out and fast-path state decrypt fails.

## What is working
- Auth bypass: manager accepts `DEV-MANAGER-TOKEN` (`authorize_attach accepted via dev bypass token`).
- Attach succeeds: `attach_by_code verification succeeded ... session_id=ddff98de-...`.
- Controller forwarder starts; leases issued.

## What is failing
- WebRTC: repeated `pingAllCandidates called with no candidate pairs` and `could not listen udp 172.18.0.8: invalid port number` followed by `timeout waiting for data channel` → fallback to WS.
- Fast-path state parse: `failed to parse state message from fast-path channel ... invalid type: string ...` which surfaces as browser `authentication tag mismatch`.
- Manager rebuild churn: bind-mount causes cargo rebuilds on each run, log windows get wiped; many curls hit resets unless manager is fully up.

## Capturing the fast_path hint (required before fixing)
1) Ensure manager is built and running (no cargo output). Check: `docker compose logs beach-manager --tail=20` should show `Running target/debug/beach-manager` and `Starting Beach Manager on 0.0.0.0:8080`.
2) Use IPv4 to avoid ::1 resets and attach the target session:
   ```bash
   curl -4 -s -o /tmp/attach.out -w '%{http_code}' \
     'http://127.0.0.1:8080/private-beaches/66e43c7e-f575-4780-8cff-38a48a7e0cdc/sessions/attach-by-code' \
     -H 'Authorization: Bearer DEV-MANAGER-TOKEN' \
     -H 'Content-Type: application/json' \
     --data-raw '{"session_id":"ddff98de-16c3-4897-ae2c-ef20eeb3dd33","code":"LNJ0DH"}'
   ```
3) Immediately pull logs for the handshake and hint:
   ```bash
   docker compose logs beach-manager --tail=200 | rg "manager handshake prepared|fast_path|attach_by_code"
   ```
   You should see `manager handshake prepared ... fast_path_hint=...` with `offer_path`, `ice_path`, and channel labels (`mgr-actions/acks/state` by default).

## Likely root causes to fix
- **UDP bind error**: `could not listen udp 172.18.0.8: invalid port number` and STUN bind failures prevent the data channel from opening. Verify ICE port range env (`BEACH_ICE_PORT_START/END=62000-62100`) and that the address isn’t being mis-parsed. Consider disabling IPv6/forcing IPv4 STUN or using host candidates via `host.docker.internal`.
- **Hint/label mismatch**: Ensure browser/host uses the manager-provided channels. Defaults are:
  ```
  offer_path: /fastpath/sessions/<session_id>/offer
  ice_path:   /fastpath/sessions/<session_id>/ice
  channels: { actions: "mgr-actions", acks: "mgr-acks", state: "mgr-state" }
  ```
  If logs show different labels on host, align them or adjust hint.
- **Fast-path parsing**: The `invalid type: string ...` errors indicate decrypt/parse failure of state messages. Once the data channel opens with consistent keys, these should disappear.

## Stabilizing the environment
- Bring compose up once with dev envs and leave it running (no code edits to avoid rebuild):
  ```bash
  DEV_ALLOW_INSECURE_MANAGER_TOKEN=1 \
  DEV_MANAGER_INSECURE_TOKEN=DEV-MANAGER-TOKEN \
  PRIVATE_BEACH_MANAGER_TOKEN=DEV-MANAGER-TOKEN \
  NEXT_PUBLIC_PRIVATE_BEACH_MANAGER_TOKEN=DEV-MANAGER-TOKEN \
  PRIVATE_BEACH_BYPASS_AUTH=0 \
  BEACH_SESSION_SERVER=http://beach-road:4132 \
  PONG_WATCHDOG_INTERVAL=10.0 \
  direnv exec . sh -c 'docker compose down && docker compose up -d'
  ```

## End-to-end verification (after fix)
1) Reattach via curl (above) and confirm:
   - No `authentication tag mismatch` in browser.
   - Manager logs show fast-path connected (no parse errors).
2) Run Playwright headful to prove both legs (manager↔host, browser↔host):
   ```bash
   SKIP_PLAYWRIGHT_WEBSERVER=1 \
   PRIVATE_BEACH_ID=66e43c7e-f575-4780-8cff-38a48a7e0cdc \
   CLERK_USER=test@beach.sh CLERK_PASS='h3llo Beach' \
   direnv exec . sh -c 'cd apps/private-beach && npx playwright test tests/e2e/pong-fastpath-live.pw.spec.ts --project=chromium --headed'
   ```
   Assert tiles render and no fast-path errors in the UI logs.

## What to do next
- Get a stable manager run, capture the emitted `fast_path_webrtc` hint, and fix either the ICE bind issue (likely primary) or any channel-label mismatch. Once WebRTC data channel opens, the tag mismatch should clear. Then rerun Playwright to prove fast-path works end-to-end.

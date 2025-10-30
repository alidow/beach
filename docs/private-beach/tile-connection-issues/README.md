# Tile Connection Instability – Investigation Log

_Last updated: 2025-03-31_

> **TL;DR**  
> Layout churn (React Strict Mode double-invoke + tile measurement updates) continues to
> unmount/remount the entire terminal preview tree. Each remount still restarts the viewer hook,
> even after the reuse refactor and the new connection cache. The hook now frequently restores the
> cached transport **without** reattaching listeners or the store snapshot, so we end up with a
> connected WebRTC channel that never emits frames. We need a durable connection manager that
> survives component unmounts, plus explicit listener reattachment and viewport sync on every
> mount. Once that is stable we can use the sandbox Playwright spec to prevent regressions.

---

## 1. Background

- **Surface:** Private Beach dashboard (`apps/private-beach/src/pages/beaches/[id]/index.tsx`)
- **Tile renderer:** `SessionTerminalPreview` & `SessionTerminalPreviewClient`
- **Transport:** Beach Surfer WebRTC client (`apps/beach-surfer`)
- **Hook:** `useSessionTerminal` establishes the viewer connection and owns the terminal grid store.

### Observed Symptoms

1. Initial bug: single-tile layouts (e.g. loading the “private beach” view with one session) would
   repeatedly reconnect, clogging `/sessions/:id/controllers` and `/webrtc/offer` with 403/404
   errors.
2. After initial fixes, the preview stabilised but still reconnected a few times before settling.
3. Latest reuse attempt (per-mount listener bundle + connection cache) still leaves the preview
   blank. Logs show the data channel opening, but the store never sees `frame` events after the
   component remounts. Sandbox view mirrors production: “Preparing terminal preview…” never clears.

---

## 2. Timeline & Changes

| Step | Change | Result |
| ---- | ------ | ------ |
| A | Added rich logging across `CanvasSurface`, `TileCanvas`, `useSessionTerminal`, and Beach Road/Manager. | Identified measurement loop as primary culprit. |
| B | Allowed viewer tokens to read controller pairings (server change). | Removed 403s but not the loop. |
| C | Persisted tile measurement metadata in `ensureLayoutMetadata` and API loader. | Measurement version now preserved. |
| D | Short-circuited `handleTileMeasurements` when measurement unchanged. | Loop persisted because parent layout still cloned. |
| E | Added guard in `updateLayout` to skip no-op clones. | Eliminated measurement-driven remounts. |
| F | Introduced reuse logic in `useSessionTerminal` to keep existing WebRTC connection if deps unchanged. | **Regression:** Hook cleanup removed event listeners before reuse path exited, leaving transport open but inert. |
| G | Added decision logging + strict-mode guard + in-memory connection cache to survive remounts. | **Current:** Dev/prod still remount the tree; cached transport is reused but listeners/store never rebind → blank preview. |

Key log snippets (from `temp/private-beach.log`):

- `canvas-measure` transitions from `apply` to frequent `skip` events (good).
- After change F, `effect-start` logs disappear; only `fetch-viewer-credential` runs once, but
  no `transport-open` events follow when layout changes.
- The terminal remains blank because `cleanupListeners` already detached `frame` handlers before
  the effect short-circuited.

---

## 3. Current State

1. `SessionTerminalPreviewClient` unmounts repeatedly:
   - React Strict Mode in dev intentionally mounts → unmounts → remounts.
   - Canvas layout tweaks (resize, drag, viewport measurement) also trigger unmount/remount cycles.
2. The hook now consults an in-memory connection cache. When the component remounts it finds the
   cached transport and returns early, but the listener bundle (and grid store viewport) are not
   reattached, so we never process `frame` events.
3. Result: WebRTC connection is stable (no reconnect spam), yet terminal content stays blank and
   the placeholder never clears.
4. Sandbox page (`/dev/private-beach-sandbox`) with `skipApi=1` and a static terminal fixture shows
   the same behaviour, providing a fast repro without manager services.

---

## 4. Proposed Solution

### Goal

Zero restarts and live terminal feed even when layout updates happen. New connection only when:

- Session ID / manager URL / auth token / viewer credential overrides change.
- Transport actually closes (`close` event or fatal error).

### Plan (rev 2)

1. **Extract a connection manager module** shared across mounts.
   - Keyed by `{sessionId, managerUrl, token, overrides}`.
   - Owns the live `BrowserTransportConnection`, refcount, keep-alive timeout, and listener bundles.
   - Provides `acquire()` / `release()` so React components only bind/unbind listeners.
2. **Rebind listeners + store on every mount.**
   - Even when reusing a cached connection, always attach `frame`, `open`, `close`, etc. handlers and
     resynchronise the `TerminalGridStore` viewport.
   - Ensure Strict Mode cleanup simply calls `release()` without tearing down the shared transport.
3. **Stabilise viewport + measurements.**
   - When remounting, immediately resend the last known viewport to avoid the preview entering an
     empty state before frames resume.
4. **Automated repro:**
   - Finish Playwright spec (`apps/private-beach/tests/e2e/private-beach-sandbox.spec.ts`) that loads
     the sandbox fixture and asserts the banner text renders before any interactions.
   - Extend spec to drag/resize the tile and confirm the content persists (no reconnect placeholder).

Once this architecture is in place we can remove most of the ad-hoc logging and rely on the spec +
tile telemetry for regressions.

---

## 5. Suggested Implementation Steps

1. Refactor `useSessionTerminal`:
   - Replace `cleanupListeners` array with a ref (`listenersRef`) storing the detach lambdas.
   - On reuse, call `listenersRef.current?.reattach(connection)` (factory returning attach/detach).
   - Ensure cleanup invoked with `{closeConnection: false}` does **not** call existing detach functions.
2. After refactor, exercise the dashboard:
   - Confirm logs show `attach-listeners` on initial connect, no `effect-cleanup` unless real change.
   - Terminal frames populate `TerminalGridStore`.
3. Remove temporary debugging logs once behaviour verified (optional but recommended).

---

## 6. Open Questions

- Should we debounce or batch canvas measurement updates to cut down on layout churn further?
- Do we need a dedicated store per tile (currently re-created on every render but reused via
  `useMemo` with `sessionId`)?
- Any multi-tile implications? (We only tested single-tile; multi-tile may still churn.)

---

## 7. Next Actions for New Engineer

1. Build the shared connection manager (see §5 rev 2) and migrate `useSessionTerminal` to it.
2. Verify in sandbox:
   - Load `/dev/private-beach-sandbox?skipApi=1&sessions=sandbox-session|application|Sandbox Fixture&terminalFixtures=sandbox-session:pong-lhs`.
   - Confirm placeholder clears and fixture banner renders.
   - Drag/resize the tile; ensure no reconnect placeholder and log shows reuse path.
3. Run Playwright spec `private-beach-sandbox.spec.ts` (with the dev server running on port 3000).
4. Re-test production dashboard against real manager to confirm live frames flow.
5. Update this log again once the fix is validated (expect to remove the placeholder warning).

### Reproduction Commands

```
# Start dev server on a clean port (ensure 3000 free):
npm --prefix apps/private-beach run dev -- --hostname 127.0.0.1 --port 3000

# In another shell, run the sandbox Playwright spec with explicit timeout:
DEBUG=pw:api PW_TEST_HTML_REPORT=0 npx --yes playwright test private-beach-sandbox.spec.ts \
  --config apps/private-beach/playwright.config.ts \
  --project=chromium \
  --timeout=90000 \
  --workers=1 \
  --retries=0 \
  --reporter=list
```

Playwright currently hangs if the dev server isn’t ready; run the dev server first or switch to a
Next.js production build to speed up startup.

---

## 8. Quick Reference

- **Key files:**
  - `apps/private-beach/src/hooks/useSessionTerminal.ts`
  - `apps/private-beach/src/components/CanvasSurface.tsx`
  - `apps/private-beach/src/components/TileCanvas.tsx`
- **Logs:** `temp/private-beach.log` (client), `temp/beach-surfer.log` (transport)
- **Manager/Road services:** via docker-compose (`beach-manager`, `beach-road`)

---

Please keep this document updated as new findings emerge.

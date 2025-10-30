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
| G | Added decision logging + strict-mode guard + in-memory connection cache to survive remounts. | Partial – cache keeps channel open but listeners/store still lost on remount (blank preview). |
| H | Replaced `useSessionTerminal` with shared connection manager + per-key terminal store. Added sandbox Playwright spec. | Sandbox fixture renders reliably and survives interactions; need to validate against live manager sessions. |

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
2. Connection manager now owns the WebRTC transport + `TerminalGridStore` per key. React mounts just
   subscribe/unsubscribe; the transport stays alive across remounts. Sandbox fixture (Playwright) verifies
   the placeholder clears and content persists on click.
3. Still outstanding: confirm live manager sessions stream frames with the new manager, and sniff logs for
   any missed listener detaches.

---

## 4. Proposed Solution

### Goal

Zero restarts and live terminal feed even when layout updates happen. New connection only when:

- Session ID / manager URL / auth token / viewer credential overrides change.
- Transport actually closes (`close` event or fatal error).

### Plan (rev 2)

1. ✅ **Connection manager**: implemented in `sessionTerminalManager.ts`. React components now subscribe
   to a per-key entry that owns the transport, listeners, reconnection loop, and terminal store. Strict Mode
   cleanup simply decrements a refcount; keep-alive timers close idle transports after 15s.
2. ✅ **Listener/store reuse**: every subscriber receives the same `TerminalGridStore`, and listeners are
   attached exactly once per connection. Heartbeat latency updates continue to flow to hooks.
3. ✅ **Automated repro**: sandbox Playwright spec (`private-beach-sandbox.spec.ts`) loads a static fixture,
   waits for the placeholder to disappear, and verifies the banner text survives a tile click.
4. ⏳ **Live session validation**: still need to exercise the dashboard against real manager + surfer,
   confirm frame streaming, and ensure placeholders disappear under resize/drag.

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

1. ✅ Connection manager deployed; hook now delegates lifecycle to it (`sessionTerminalManager.ts`).
2. ✅ Sandbox validation passes via Playwright spec.
3. ⏳ Exercise real manager sessions to confirm frames stream and latencies update after drag/resize.
4. ⏳ Iterate on additional Playwright coverage (e.g., drag/resize once transporter fix confirmed).

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

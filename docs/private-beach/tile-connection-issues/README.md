# Tile Connection Instability – Investigation Log

_Last updated: {{DATE}}_

> **TL;DR**  
> When the private beach dashboard renders a tile, repeated layout measurements force the
> `SessionTerminalPreview` tree to unmount/remount. Every remount restarts the WebRTC viewer hook
> (`useSessionTerminal`). Our attempts to suppress those restarts partially succeeded, but the
> latest optimisation leaves the connection open without any event listeners, so the terminal
> never receives frames. We need to restructure the hook so that listener lifecycles are de‑
> coupled from React renders and to ensure measurement updates stop at the outer layout layer.

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
3. Latest change (connection reuse) removed restarts, but the terminal now displays nothing—the
   channel is open but no frames arrive.

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

Key log snippets (from `temp/private-beach.log`):

- `canvas-measure` transitions from `apply` to frequent `skip` events (good).
- After change F, `effect-start` logs disappear; only `fetch-viewer-credential` runs once, but
  no `transport-open` events follow when layout changes.
- The terminal remains blank because `cleanupListeners` already detached `frame` handlers before
  the effect short-circuited.

---

## 3. Current State

1. `useSessionTerminal` effect now:
   - Builds a dependency signature.
   - If signature matches and the connectionRef is non-null, it returns early (`shouldReuseConnection`).
   - **Issue:** cleanup ran first, cleared listeners, and we skip the block that reattaches them.
2. Connection is open (no reconnect spam) but no handlers → no terminal frames processed.
3. Measurement updates still happen legitimately (when host grows from 24→62 rows). These should
   adjust layout without restarting the viewer.

---

## 4. Proposed Solution

### Goal

Zero restarts and live terminal feed even when layout updates happen. New connection only when:

- Session ID / manager URL / auth token / viewer credential overrides change.
- Transport actually closes (`close` event or fatal error).

### Plan

1. **Split effect responsibilities:**
   - Keep a stable effect that owns the WebRTC transport lifecycle.
   - Use refs to store handlers; only re-register listeners on actual transitions (closed → open).
2. **Refine cleanup:**
   - If we intend to reuse the connection, skip both `closeCurrentConnection()` and listener
     deregistration (`cleanupListeners.length = 0`).
   - Add explicit reuse path to re-register listener callbacks after early return (if we keep the
     current structure).
3. **Introduce explicit `attachListeners(connection)` helper:**
   - Registers `open`, `secure`, `frame`, `close`, `error`, `status`.
   - Stores detach functions in ref so we can call them only when the connection truly closes.
4. **Gate layout update logging:**
   - Keep `[canvas-update]` logs, but include `skipped` boolean (already added) to confirm no new
     remount churn.
5. **Regression guard:**
   - Write a Playwright or unit test that ensures streaming frames persist after repeated layout
     measurement emissions (simulate measurement updates without closing the transport).

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

1. Implement listener lifecycle refactor described in §5.
2. Validate the change by:
   - Running the dashboard on a single tile.
   - Checking `temp/private-beach.log` for absence of repeated `effect-start`/`effect-cleanup`.
   - Ensuring terminal text appears without blank states.
3. Draft regression tests if time permits.

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

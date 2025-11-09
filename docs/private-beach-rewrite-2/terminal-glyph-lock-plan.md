# Tile Terminal Stabilization Plan

## Objective
Reproduce the exact rendering behavior of BeachTerminal in the standalone beach-surfer client when the component is embedded inside canvas tiles. Specifically:
1. **Eliminate DOM-based glyph measurements inside transformed tiles.** Provide per-cell metrics via props sourced from a hidden, unscaled glyph probe.
2. **Lock BeachTerminal to host PTY dimensions** when requested so DOM width/height never trigger logical reflow. The tile merely crops/scrolls.
3. **Ensure tile-driven actions never resize the host PTY.** Double-click auto-resize changes only the tile’s dimensions; host stays fixed.
4. **Add instrumentation** so if any assumption fails (missing PTY dimensions, missing glyph metrics), we know immediately why.

## Root Causes (Validated)
- **Scaled measurements.** `useLayoutEffect` samples `.xterm-row` via `getBoundingClientRect`. Inside React Flow, those coordinates include the node’s `transform` (scale + fractional translate), so `--beach-terminal-cell-width`/`line-height` get overwritten with distorted values. DOM dumps show line heights jumping to ~445 px.
- **Viewport feedback loop.** Even with `autoResizeHostOnViewportChange=false`, BeachTerminal still updates `viewportCols` from `container.clientWidth / cellWidth`. When the tile shrinks, the logical grid shrinks. Host PTY is unaffected, but the client now renders fewer columns, so the horizontal border never reaches the vertical bars.
- **Tile auto-resize guessed from DOM.** Double-click handler measures the DOM width/height of the terminal inside the transformed tile and uses that to compute new tile dimensions, compounding errors.

## Solution Overview
| Area | Change |
| --- | --- |
| Glyph metrics | Provide via prop (`cellMetrics`). New `TerminalGlyphProbe` component renders in a portal outside React Flow, waits for fonts to load, measures a glyph, and passes `{ widthPx, heightPx }` back. When `cellMetrics` is supplied BeachTerminal must never query `.xterm-row`. |
| Viewport lock | Add `lockViewportToHost` mode. When true: (a) `computeViewportGeometry` returns host cols; (b) `resolveHostViewportRows` returns host rows; (c) DOM-based `scheduleViewportCommit`/debounce/ResizeObserver paths are disabled; (d) BeachTerminal only reflows when host PTY reports a different size. |
| Tile wrapper | Wrap BeachTerminal in fixed-size div (`overflow: hidden auto`). Tile size equals `hostPixelWidth/Height + chrome offsets`. Scrolling/cropping handled by wrapper. |
| Auto-resize | `computeAutoResizeSize` now requires `hostPixelWidth/Height`. Double-click aborts (with telemetry) if metrics missing. Never send host resize frames. |
| Diagnostics | Console warnings under `window.__BEACH_TRACE` when host metrics missing, glyph metrics missing, or tile attempts to resize without required data. |

## Detailed Implementation Steps
### 1. BeachTerminal changes (apps/beach-surfer/src/components/BeachTerminal.tsx)
1. **Props**
   - `cellMetrics?: { widthPx: number; heightPx: number }`
   - `lockViewportToHost?: boolean` (default false)
2. **Measurement logic**
   - If `cellMetrics` present, set `measuredCellWidth`/`measuredLineHeight` from props and skip all DOM probes.
   - If `lockViewportToHost` true and `cellMetrics` missing, rely exclusively on `measureFontGlyphMetrics` (offscreen probe) and *never* query `.xterm-row`.
3. **Viewport control**
   - `computeViewportGeometry` returns `{ targetCols: hostCols ?? DEFAULT_TERMINAL_COLS }` when locked; measuredCols remains `null`.
   - `resolveHostViewportRows` returns host rows (`ptyViewportRowsRef`) when available; until then, fallback 80×24 but log `[lock-viewport] missing-host-rows` once.
   - Guard `scheduleViewportCommit`, `domPendingViewport*`, and `disableViewport` toggles behind `!lockViewportToHost` so DOM width changes are ignored.
4. **State emission**
   - Extend `TerminalViewportState` with `hostPixelWidth`, `hostPixelHeight`, `cellWidthPx`, `cellHeightPx`.
   - Emit diagnostics when host PTY hasn’t reported its size after handshake (helps trace host bugs).

### 2. Glyph probe component
- Create `apps/beach-surfer/src/components/TerminalGlyphProbe.tsx`:
  - Uses `ReactDOM.createPortal` to render offscreen.
  - Waits for `document.fonts.ready` before measuring.
  - Calls `onMetrics({ widthPx, heightPx })` once; cleans up DOM node afterward.
- Export hook `useTerminalGlyphMetrics(fontFamily, fontSize)` that manages probe lifecycle and returns cached metrics.

### 3. Rewrite-2 tile integration
1. **SessionViewer**
   - Call `useTerminalGlyphMetrics` (or provide metrics from parent). Pass `cellMetrics` + `lockViewportToHost` to BeachTerminal. Ensure `disableViewportMeasurements` stays true for tiles.
   - Wrap BeachTerminal with a fixed-size div (`overflow: auto`) so cropping is purely CSS.
2. **ApplicationTile / TileFlowNode**
   - Store latest `hostPixelWidth/Height` + `cellWidthPx/Height` in `viewportMetricsRef`.
   - Remove any DOM-based measurement (getBoundingClientRect) from auto-resize path.
   - Ensure double-click handler aborts unless host metrics ready; log `[tile][auto-resize] missing-host-metrics` when absent.

### 4. Auto-resize utility
- Update `computeAutoResizeSize` to require host pixel metrics (throw away DOM fallbacks). Use `cellWidthPx * hostCols` if `hostWidthPx` missing, but only if metrics were provided explicitly via props (not DOM measurement).
- Add unit tests covering: (a) host metrics available → expect width/height match; (b) metrics missing → returns null.

### 5. Diagnostics / Telemetry
- `window.__BEACH_TRACE` in BeachTerminal logs:
  - When host metrics missing under lock (`missing-host-rows`, `schedule-missing-host-rows`).
  - When glyph probe metrics are applied (`cell-metrics:source=props` or `font-probe`).
- `window.__BEACH_TILE_TRACE` logs tile-level events: glyph metrics ready, auto-resize attempt, host metrics missing, etc.
- Telemetry events for auto-resize (`canvas.resize.auto`) include host pixel metrics and whether resize was skipped.

## Red-Team Considerations & Mitigations
| Risk | Mitigation |
| --- | --- |
| Fonts not loaded when probe runs | Await `document.fonts.ready`, retry with exponential backoff, fallback to default metrics with warning. |
| Host PTY never reports size | New diagnostics make this obvious; tile should display a banner (future work) if host size missing.
| BeachTerminal still used in contexts that rely on DOM sizing | `cellMetrics` and `lockViewportToHost` default off; existing behavior unchanged for beach-surfer.
| CSS padding/margins cause tile crop mismatch | Document tile chrome offsets and add helper to compute `contentWidth = tile.width - chromeWidth`.
| Double-click still resizes host | Explicitly remove/send no host resize frames; add telemetry assert when resize frame would have been sent.

This plan provides enough detail that another engineer can implement it end-to-end: add the glyph probe, extend BeachTerminal props, wire host-lock mode, update tiles, and add instrumentation/tests. EOF

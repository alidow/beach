# Tile-Terminal Stability Plan

## Background
- **Current behavior:** BeachTerminal measures glyph metrics by sampling `.xterm-row` inside the tile. React Flow wraps each tile in a `transform: matrix(...)`, so those measurements are scaled/translated before BeachTerminal writes them back to `--beach-terminal-cell-width` and `--beach-terminal-line-height`. The component then re-renders rows at those distorted sizes.
- **Viewport feedback loop:** BeachTerminal still derives `viewportCols` from `container.clientWidth / cellWidth` and occasionally requests host PTY resizes when the viewport changes. In the canvas this produces reflow instead of simple cropping.
- **Goal:** Make the tile behave like the beach-surfer web client: BeachTerminal renders the host grid at fixed per-cell pixels, the tile simply crops/scrolls, and double-click auto-resize adjusts ONLY the tile, never the host PTY.

## Root Cause Analysis (Validated)
1. **Scaled glyph measurements** – DOM probing happens inside the transformed subtree. Even at zoom 1.0 the transform introduces fractional offsets, so successive measurements drift and set `--beach-terminal-line-height` to hundreds of pixels (dom dumps confirm).
2. **Host PTY resize loop** – Even though `autoResizeHostOnViewportChange` defaults to `false`, BeachTerminal still recomputes `viewportCols` from the container width and rewrites the logical grid. The host PTY is not resized, but the client grid is, which is why we see the “smooshed” horizontal borders.
3. **Tile’s DOM width is authoritative** – The tile continuously feeds DOM width back into BeachTerminal. Thus every drag/zoom results in BeachTerminal truncating/expanding the rendered columns instead of just clipping via CSS overflow.

## Proposed Solution
The fix has 4 pillars. Each step must be implemented entirely; partial fixes reintroduce the bug.

### 1. External glyph metrics (no DOM sampling inside tiles)
- Introduce a new optional prop `cellMetrics?: { widthPx: number; heightPx: number }` on `BeachTerminalProps`.
- When this prop is provided, BeachTerminal must **never** query `.xterm-row` or run `getBoundingClientRect`. It should feed the provided metrics into `pixelsPerRowRef/pixelsPerColRef` and `--beach-terminal-*` CSS vars.
- Implement `BeachTerminalGlyphProbe` (React component) that renders off-screen (via portal to `document.body`) and measures a span before the tile is mounted. It waits for `document.fonts.ready` to avoid race conditions. Tiles will use this probe once and cache results in state / context.

### 2. Lock viewport to host PTY when requested
- Add `lockViewportToHost` logic:
  - `computeViewportGeometry` returns host cols immediately; ignore DOM widths.
  - `resolveHostViewportRows` returns host rows (from PTY `grid` frames). Until host dimensions arrive, render the default 80×24, but once set never revert.
  - Skip `scheduleViewportCommit` / DOM debouncing when locked. Instead, rerun only when host reports a new PTY size or when `forcedViewportRows` changes.
  - Emit `[beach-terminal][lock-viewport] missing-host-rows` logs if the PTY never provided its size, so we can debug handshake issues.

### 3. Tile wrapper handles cropping only
- Tiles wrap BeachTerminal in a fixed pixel-sized div with `overflow: hidden auto;`. That div is sized from tile state (React Flow node width/height) and is the *only* thing that changes when a user drags/zooms.
- No DOM feedback: Remove any code that feeds `clientWidth` back to BeachTerminal or that requests host resize during drag. The only way to change host PTY size is via explicit user action (Match Host button if we add one later).

### 4. Auto-resize tile using host pixel metrics
- Update `computeAutoResizeSize` to demand `hostWidthPx`/`hostHeightPx`. If they are missing, log `[tile][auto-resize] missing-host-metrics` and abort (no fallback to DOM width).
- Double-click should **only** set `tile.size` to `(hostWidthPx + chromeWidth, hostHeightPx + chromeHeight)`; never send `{ type: 'resize', cols, rows }` to the host.

## Implementation Steps
1. **BeachTerminal updates**
   - Extend `BeachTerminalProps` with `cellMetrics` and `lockViewportToHost`.
   - Refactor measurement hooks: early-return entirely when `cellMetrics` is provided or `lockViewportToHost` is true.
   - Ensure `emitViewportState` includes `hostPixelWidth`/`hostPixelHeight` derived from `pixelsPerCol * hostCols`.
   - Guard all resize hooks (ResizeObserver-based) behind `!lockViewportToHost`.
   - Add diagnostics (`logLockViewportDiagnostic`) for missing host metrics.

2. **Glyph probe**
   - Create `apps/beach-surfer/src/components/TerminalGlyphProbe.tsx` (shared) that renders off-screen, measures `span` width/height once fonts are ready, and invokes a callback with `{ width, height }`.
   - Export helper hook `useTerminalGlyphMetrics(fontFamily, fontSize)` that tiles can call before mounting BeachTerminal.

3. **Tile integration**
   - In rewrite-2 `SessionViewer`, call the glyph probe hook (or use context provided by `TileFlowNode`) and pass `cellMetrics` + `lockViewportToHost` into BeachTerminal.
   - Wrap BeachTerminal in a fixed-size div with `overflow: auto` (existing structure already close; ensure no additional padding alters the calculation).
   - Ensure tiles stop passing `disableViewportMeasurements={false}`; they should rely exclusively on the host-lock path.

4. **Auto-resize refactor**
   - Update `computeAutoResizeSize` to require host pixel metrics. Add logging when they’re absent.
   - `TileFlowNode.handleAutoResize` picks up `hostWidthPx/hostHeightPx` from last viewport snapshot and resizes the tile, never the host PTY.

5. **Diagnostics**
   - Add `window.__BEACH_TILE_TRACE` logging: dump host metrics, tile chrome sizes, and whether lock mode is active.
   - Add warnings when tiles try to auto-resize without host metrics.

## Red-Team / Risks
- **Font probe timing:** Measuring outside the tile avoids transforms but still fails if fonts aren’t ready. Mitigation: await `document.fonts.ready` and retry on failure.
- **Host metrics never arrive:** Lock mode depends on PTY sending `cols/viewportRows`. Diagnostics are added, but the UX still degrades. Consider a timeout to fall back to DOM sizing with a prominent warning.
- **Legacy code paths:** Other consumers (beach-surfer web client) still rely on DOM measurement. Ensure new props are optional and default to existing behavior to avoid regressions.
- **Resize observers still firing in lock mode:** Double-check every effect (e.g., `scheduleViewportCommit`, disable/enable measurements) is gated by `lockViewportToHost`; missing one will reintroduce the bug.
- **Tile chrome padding:** When we size the tile to host pixels, remember to add the fixed chrome offsets (header, padding). Document the math so future devs don’t miscalculate.

This plan removes the two feedback paths (DOM measurement & viewport reflow) that cause the current instability while keeping beach-surfer behavior unchanged. EOF

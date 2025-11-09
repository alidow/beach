# Tile Terminal Stabilization Plan

## Objective
Locked BeachTerminal instances inside React Flow tiles already avoid DOM-driven viewport logic. Remaining issues are glyph metrics that occasionally drift (fonts not loaded, DPR changes, SessionViewer quantization mismatch) and missing diagnostics when metrics fall back. This plan tightens that lifecycle without reimplementing features we already ship (lock mode, host-metric auto-resize, PTY protection).

## Current State Check
- `lockViewportToHost` is wired end-to-end; DOM resize observers and `.xterm-row` probes no-op when locked (`apps/beach-surfer/src/components/BeachTerminal.tsx`).
- `TerminalViewportState` already exposes `pixelsPerRow`, `pixelsPerCol`, `hostPixelWidth`, `hostPixelHeight`; rewrite-2 consumes them for auto-resize.
- Double-click auto-resize operates purely on host metrics; no host PTY resize frames are sent.
- SessionViewer adds a quantization effect to snap `--beach-terminal-cell-width` to the React Flow scale/device pixel ratio, but BeachTerminal is unaware of that post-process.

## Gaps Observed
1. **Font readiness / re-measurements** – `measureFontGlyphMetrics` can run before fonts load and doesn’t re-run when devicePixelRatio changes while locked.
2. **Metric provenance** – We cannot distinguish “props vs probe vs fallback”, which makes debugging hard.
3. **Quantization mismatch** – SessionViewer may rewrite the CSS variable after BeachTerminal measures, so `pixelsPerCol` emitted to tiles can drift from what the DOM actually renders.
4. **Instrumentation** – We lack precise tracing for “fonts never resolved”, “using fallback metrics”, or “host rows missing under lock”.

## Plan of Record

### 1. BeachTerminal metric lifecycle
1. Add optional `cellMetrics?: { widthPx: number; heightPx: number }`. When provided we immediately set `measuredCellWidth`/`measuredLineHeight`, skip probes, and log `[beach-terminal][cell-metrics] source=props`.
2. When locked and no `cellMetrics`, upgrade the existing probe:
   - Await `document.fonts?.ready` with a 5 s timeout + retry/backoff, then fall back to baked metrics (log `source=fallback`).
   - Re-run the probe whenever `window.resize`, `visualViewport` events, or `matchMedia('(resolution)')` fire so zoom/DPR changes propagate.
   - Every successful measurement logs `source=font-probe` plus duration.
3. After each measurement (whether from props or probe) read back `--beach-terminal-cell-width` via `getComputedStyle(container)` when locked; if it differs from `measuredCellWidth`, update `pixelsPerColRef` and emit a viewport state so host pixel metrics match whatever quantization did.
4. Keep the DOM `.xterm-row` observer disabled in lock mode as today; no new DOM sampling is introduced.

### 2. SessionViewer alignment
1. Keep the existing quantization effect but, after it writes the CSS custom property, emit the quantized width/height through a ref so we can compare against viewport metrics (mainly for tracing).
2. Pass through the optional `cellMetrics` prop when a parent wants to supply pre-probed metrics (hook will come in a later milestone—out of scope today, but the prop unblocks future work).
3. Add `window.__BEACH_TILE_TRACE` logs whenever quantization runs or when host metrics arrive without glyph metrics.

### 3. Diagnostics & telemetry
- Add helper `logMetricSource(event, extra)` behind `window.__BEACH_TRACE` to capture `source`, `fontLoaded`, `durationMs`, and whether fallback was used.
- Emit `[beach-terminal][lock-viewport] missing-host-rows` only once per lock session (already happens) but include the metric-source snapshot for easier debugging.
- Surface quantized cell metrics inside `TileViewportSnapshot` (new optional fields `quantizedCellWidthPx`, `quantizedCellHeightPx`). Auto-resize telemetry should include both the host-derived and quantized widths so we can spot discrepancies.

### 4. Progress Tracker

| Task | Owner | Status |
| --- | --- | --- |
| Add `cellMetrics` prop + metric source logging in `BeachTerminal` | Codex | ☑ |
| Implement font-ready wait + DPR refresh loop | Codex | ☑ |
| Sync `pixelsPerCol` with quantized CSS var when locked | Codex | ☑ |
| Extend SessionViewer instrumentation for quantization + optional `cellMetrics` pass-through | Codex | ☑ |
| Update telemetry payloads/tests for new metric fields | Codex | ☐ |

Progress rows will flip to ☑ as we land changes.

## Red-Team Notes
- **Fonts never resolve** – timeout falls back to conservative metrics and logs `[beach-terminal][cell-metrics] source=fallback`.
- **Multiple tiles** – Optional `cellMetrics` lets a higher-level cache distribute metrics so we don’t probe per tile in the future.
- **Drift between host metrics & rendered CSS** – Reading back the CSS variable after quantization keeps telemetry honest even under fractional React Flow scales.

EOF

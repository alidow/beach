# Private Beach Tile Sizing & Terminal Alignment — Root Cause Analysis

## Scope

Diagnoses two regressions reported after the tile canvas refactor:

1. **Unlocked tiles still span the full dashboard width on first render.**
2. **Terminal preview shows broken vertical borders/continuous scroll drift.**

The analysis below traces each issue to the exact mutations in `apps/private-beach/src/components/TileCanvas.tsx` and `apps/beach-surfer/src/components/BeachTerminal.tsx`, confirms the behaviour with the current trace logs (`[tile-layout] measure … width: 1499`, broken glyph rows), and outlines the minimal fixset required to restore the intended UX.

---

## Issue A — Tiles revert to full-width (≈1500 px)

### Symptoms
- `[tile-layout] ensure … {"w":3,"h":3}` confirms the React Grid Layout (RGL) data still requests a 3-column tile.
- `[tile-layout] measure … {"width":1499,"height":…}` records the DOM width at ≈1499 px, matching the dashboard container, and the tile renders stretched.
- Resizing manually works until the next reload, at which point the tile snaps back to the wide state.

### Root Cause
During the refactor we introduced `gridWidth` and updated `handleMeasure` to persist tile measurements:

```ts
const normalized: TileMeasurements = {
  width: measurement.width,
  height: measurement.height,
};
```

On the first ResizeObserver callback:
- `gridWidth` is still `null`.
- `measurement.width` is the raw DOM width (**1499 px**) because the wrapper still spans 100 % of the dashboard.
- We now persist that raw width into `tileState.measurements`.
- Subsequent zoom calculations (`computeZoomForSize`) use the cached 1499 px width, so the tile keeps the full width even though `w === 3`.

Previously, the code either clamped unlocked tiles to `UNLOCKED_MAX_W` or derived width from layout units, preventing the wide measurement from being stored. The regression is therefore **the missing guard around the measurement write-back**.

### Proposed Fix
Only persist a measurement once we can map it to the RGL column width:

```ts
const layoutItem = layoutMap.get(sessionId);
const widthFromLayout =
  layoutItem && gridWidth != null && cols > 0
    ? (gridWidth / cols) * layoutItem.w
    : null;
if (!widthFromLayout) {
  return; // skip until we know the column width
}
const normalized = { width: widthFromLayout, height: measurement.height };
updateTileState(…);
```

This guarantees that the stored width reflects the intended 3-column span (~360–400 px) and prevents the full-width snap-back on reload.

Verification: reload with trace logging enabled and confirm `[tile-layout] measure … width:` drops to ~360 px; tile renders at the compact size.

---

## Issue B — Terminal borders misaligned / viewport cropped

### Symptoms
- Vertical border glyphs (`│`, `┆`, custom dashed lines) show visible gaps between rows.
- Initial zoom hides ~60 % of the PTY height; scrolling drifts back to the top.

### Root Cause
The refactor now sets `--beach-terminal-cell-width` by sampling the first rendered row:

```ts
const rect = row.getBoundingClientRect();
const roundedCellWidth = rect.width / Math.max(1, snapshot.cols);
```

Two problems arise on first render:
1. The row often contains only the left border characters, not all `snapshot.cols` cells. Dividing by 80 (default) produces a cell width larger than the actual glyph width, creating the visual gaps.
2. Because the computed cell width is inflated, `computeZoomForSize` believes the terminal fills the tile horizontally and clamps the zoom to ~100 %, cropping the lower rows.

### Proposed Fix
Measure an actual cell span instead of the whole row divided by `snapshot.cols`:

```ts
const cell = row.querySelector<HTMLSpanElement>('span');
if (cell) {
  const cellWidth = cell.getBoundingClientRect().width;
  setMeasuredCellWidth(cellWidth);
} else {
  const cellCount = row.childElementCount || snapshot.cols || DEFAULT_TERMINAL_COLS;
  setMeasuredCellWidth(rect.width / cellCount);
}
```

This captures the true glyph width (≈7–8 px at the default zoom) regardless of how many columns a row currently displays. With an accurate `--beach-terminal-cell-width`, borders stitch correctly and the calculated zoom now fits the entire PTY.

Verification: after applying the fix, reload with trace logging, open DevTools → console, and confirm the first few rows render without gaps. The default tile should show the full 80×24 viewport without scrolling.

---

## Summary of Required Changes

| Component | File | Action |
|-----------|------|--------|
| Tile measurement guard | `apps/private-beach/src/components/TileCanvas.tsx` | Skip persisting `measurement.width` until we can derive `widthFromLayout`; store the layout-based width instead. |
| Terminal cell width measurement | `apps/beach-surfer/src/components/BeachTerminal.tsx` | Measure an actual glyph span (`.xterm-row span`) and fall back to row width divided by rendered cell count; update `--beach-terminal-cell-width` accordingly. |
| (Optional) Logging | same files | Confirm via `[tile-layout] measure` and inspect borders visually after reload. |

Implementing these changes restores compact default tiles and continuous terminal borders while retaining the refactored layout.

---

## Prompt for a Fresh Codex Instance

```
Context:
- TileCanvas currently stores the first ResizeObserver measurement (≈1499px) before gridWidth is populated, so unlocked tiles render full-width despite w=3.
- BeachTerminal derives --beach-terminal-cell-width from rowRect.width / snapshot.cols, which is incorrect when only the left border glyphs are rendered; vertical borders show gaps and the terminal zoom snaps to host size.

Tasks:
1. Update apps/private-beach/src/components/TileCanvas.tsx so handleMeasure waits for gridWidth/cols and uses the layout column width (gridWidth / cols * layoutItem.w) when persisting measurements. Skip persisting until that value is available, and keep the existing zoom logic.
2. Update apps/beach-surfer/src/components/BeachTerminal.tsx to measure the actual glyph width. Query the first span inside .xterm-row; if present, use its bounding box width, otherwise divide the row width by the rendered cell count (fallback snapshot.cols or 80). Feed the result into --beach-terminal-cell-width.
3. Run npm run lint inside apps/private-beach, reload the dashboard with debug logging enabled (window.__PRIVATE_BEACH_DEBUG__ = true), and confirm console output contains `[tile-layout] measure` widths ≈360–400 and terminal borders render without gaps. Record any relevant logs.
```

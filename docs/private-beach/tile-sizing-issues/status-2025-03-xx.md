# Tile Sizing Investigation — Running Status (2025-03-xx)

## Quick Snapshot

- **Symptom**: Fresh tiles inside the Private Beach dashboard still render as wide as the viewport, resize handles feel inert, and the embedded Pong terminal shows only the top portion.
- **Logs**: Client console shows `[tile-layout] ensure … w:3` and `[tile-layout] measure … width ≈ 375.75`, but React Grid Layout’s DOM still allocates every tile a `style="width: 861px"` (~7 columns on a 1503 px grid), and the helper log keeps reporting `[tile-layout] item-width missing react-grid-item`.
- **Current status**: Layout math is clamped, measurements are correct, but the rendered `.react-grid-item` never shrinks. The root cause appears to sit inside the RGL wrapper/caching; we have not yet identified why its inline width diverges from the calculated layout state.

## Timeline & What We’ve Tried

| Date | Action | Result |
| ---- | ------ | ------ |
| Initial report | Tiles hydrate at `w=3` but still span full width; terminal glyphs misaligned. | Repro confirmed with logs + screenshots. |
| Pass 1 | Added guard in `handleMeasure` to ignore invalid widths; only persist measurements after `gridWidth` exists. | Logs now show layout width ≈ 375 px, but tiles still render wide. |
| Pass 2 | Measured glyph width inside `BeachTerminal` using actual `<span>` nodes to fix dashed line gaps. | Terminal alignment improved internally, but still cropped because tile container is oversized. |
| Pass 3 | Introduced `gridWidth` state, column width calculations, and logging of `columnWidth`. | Logs confirm the math is correct (`columnWidth: 125.25`, `layoutHeight: 330`). |
| Pass 4 | Removed Tailwind `w-full` class from the tile wrapper (`TileCard`) so inline width wins. | DOM now shows no explicit class forcing width, but RGL still outputs `style="width: 861px"`. |
| Pass 5 | Clamped layout commits: both `clampLayoutItems` and cached `lastLayout` now restrict tiles to `UNLOCKED_MAX_W` until the user resizes. | `ensure` logs consistently show `w:3`; persisted layout in backend also reports `w:3`. Tile in DOM remains wide. |
| Pass 6 | Normalized height by deriving `layoutHeight = ROW_HEIGHT * h` during measurement; added `ROW_HEIGHT` constant. | Zoom math and stored heights are consistent (`height: 330`), but terminal is still visually truncated because the wrapper width remains huge. |
| Current | Restarted dev server, created brand-new Private Beach, added a single session. | Logs identical: `measure {"width":375.75,"height":330,…}`, but screenshot shows `.react-grid-item` width ≈ 861 px, and resize handles inactive. |

## Evidence & Observations

- `temp/private-beach.log` includes:
  - `[tile-layout] measure {"width":375.75,"height":330,"rawWidth":1499,"rawHeight":16,"gridWidth":1503,"cols":12,…}`
  - Repeated `[tile-layout] item-width missing react-grid-item`, indicating our post-render probe still can’t find the wrapper at the moment the effect runs.
- `temp/dom.txt` dump shows:
  - `<div class="react-grid-layout" style="height: 378px;">` without child items captured (timing issue in snapshot).
  - `react-grid-item` entries elsewhere (from `temp/tile-html.txt`) with inline `width: 861px; transform: translate(8px, 8px);`.
- Fresh tiles (new beach, new session) still inherit `width: 861px`, suggesting the LayoutProvider or RGL’s internal cache continues to set `w ~ 7` despite our clamped layout array.

## Where We’re Stuck

1. **React Grid Layout DOM width ≠ computed layout.**  
   - Layout arrays we pass to `<AutoGrid>` show `w:3`.  
   - Measurements confirm derived width ≈ 3 columns.  
   - RGL’s rendered item still holds `width: 861px`, meaning either:
     - Our `layout` prop is not the final array RGL uses (maybe internal state mutates on drag/resize or caches the initial `data-grid` attributes).
     - WidthProvider or cached layout from RGL persists a wider span before our clamps run.
     - Our logging effect runs before RGL injects its items, so we never see the actual DOM in time (`item-width missing react-grid-item`).

2. **Resize handles inactive / terminal cropped.**  
   - Because the item’s inline width is still ~7 columns, the handle can’t reduce width further (min equals current).  
   - Terminal scales to fit the oversized container (scale ≈ 0.75), so only the top of the PTY is visible.

## Suggested Next Steps for the Next Investigator

1. **Inspect RGL internal state**: instrument `AutoGrid` (WidthProvider/GridLayout) to log the final layout it keeps after `onLayoutChange`. Verify if RGL still thinks `w > 3`.
2. **Confirm initial DOM**: use a MutationObserver in `TileCanvas` to capture the first `.react-grid-item` once inserted; record its `data-grid` attribute and inline `style`.
3. **Check for cached layout**: search for localStorage/sessionStorage or API responses still returning the old widths. We saw `w:3` in logs, but we should double-check the payload coming from `/private-beaches/:id`.
4. **Evaluate WidthProvider behaviour**: ensure the enclosure passing width to RGL isn’t double-wrapping the items or overriding styles.

## Files of Interest

- `apps/private-beach/src/components/TileCanvas.tsx`  
  `handleMeasure`, `clampLayoutItems`, `handleLayoutChange`, `TileCard`.
- `apps/private-beach/src/components/SessionTerminalPreviewClient.tsx`  
  `useSessionTerminal` integration that feeds measurements & viewer state.
- `apps/beach-surfer/src/components/BeachTerminal.tsx`  
  Glyph width measurement and zoom handling.
- `temp/private-beach.log`, `temp/dom.txt`, `temp/tile-html.txt`  
  Latest captures demonstrating current behaviour.

---

## Copy/Paste Prompt for a Fresh Codex Instance

```
You are picking up the Private Beach tile-sizing investigation. First, read docs/private-beach/tile-sizing-issues/status-2025-03-xx.md to understand the history. Then explain (in simple terms) where the layout is going wrong, what the expected tile behaviour should be, and why the current implementation still renders wide/cropped tiles. Focus on the disconnect between the React Grid Layout state (w=3) and the DOM width (~861px), and outline hypotheses on how to reconcile them.
```

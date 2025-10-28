# Tile Sizing & Terminal Rendering Issues

## Overview of Current Problems

1. **Tiles Stretch to Full Width**  
   - Even after clamping grid columns, each tile still expands to the entire viewport width. The terminal content is narrower, so there is empty space on the sides.
   - Reproduced on 2025-02-14. HTML snapshot is available at `temp/dom.txt`.

2. **Terminal Unicode Misalignment**  
   - Terminal preview shows gaps between border characters (`┊`, `┆`, `╭╮`, etc.) that are not present on the host terminal.  
   - Host vs. tile captures provided in the bug report demonstrate missing rows (e.g., rows after the `▙▏` glyph) in the tile preview.

3. **Persisted Layout State**  
   - Saved layouts still record wide column spans. Even after clamping at runtime, rehydrated state reintroduces the oversized width.

4. **Testing Limitations**  
   - `npm run test -- TileCanvas` fails due to a missing optional dependency (`@rollup/rollup-darwin-arm64`), so automated regression checks aren’t currently possible.

## Diagnosis & Evidence

### Grid Width Issue
- Relevant source: `apps/private-beach/src/components/TileCanvas.tsx`.
- Logs: console output from `debugLog('tile-layout', 'ensure layout', ...)` in DevTools (`temp/private-beach.log`).
- Observation: RGL still uses the full container width because `cols` scaling does not modify the width each tile occupies (`data-grid`’s `w` value remains large or the container sets `width: 100%`). The container width itself is the culprit.

### Terminal Gaps
- Likely caused by font metrics or letter-spacing at non-default zoom levels.
- Preview client sets a scaled font size; subtle rounding or line-height adjustments cause vertical drift.
- Evidence: Side-by-side output shows missing border rows; the preview terminator (`╰`) and footer text are absent.

### Persisted Layout
- Layout snapshots include `widthPx`/`heightPx`. If stored with an oversized width, rehydration resets the container to the full width before clamps take effect.
- Evidence: `debugLog` output shows `w` values matching the maximum column count.

### Test Execution
- Failure occurs before test logic runs. No coverage of new sizing logic yet.
- Evidence: `npm run test -- TileCanvas` output (Rollup optional dependency missing).

## Where to Find Supporting Artifacts

| Artifact | Path |
| --- | --- |
| HTML snapshot (tile DOM) | `temp/dom.txt` |
| Console logs | `temp/private-beach.log` |
| Terminal baseline excerpt | Provided in user message (compare host vs. tile) |
| Source files | `apps/private-beach/src/components/TileCanvas.tsx`, `SessionTerminalPreviewClient.tsx` |

## Proposed Solutions

1. **Fix Tile Width**
   - Investigate CSS/class overrides on `.react-grid-item` to ensure its width corresponds to `w` columns rather than 100%.
   - Enforce a fixed pixel width cap via an outer container (e.g., `max-width: 400px`) or use RGL’s `draggableCancel`/`style` props to set explicit widths.
   - Recompute `cols` and update layout `w` to smaller value on first mount, then persist.

2. **Terminal Alignment**
   - Audit scaled font-size and line-height calculations in `SessionTerminalPreviewClient` and `BeachTerminal`.  
   - Ensure zoomed font metrics round to integer pixel values; adjust CSS to disable letter spacing.
   - Compare `xterm.js` theme settings to host terminal for consistent rendering.

3. **Persisted Layout**
   - On load, clamp `widthPx`/`heightPx` before setting `lastLayout`.
   - After normalization, call `onLayoutPersist` with the clamped values so the backend stores compact dimensions.

4. **Testing Pipeline**
   - Address the Rollup optional dependency by reinstalling packages (`npm ci`) or ignoring the test flag until dependencies resolve.

## Prompt for a Fresh Codex Instance

```
You’re starting from the current state of `apps/private-beach`. Review the tile sizing and terminal rendering issues documented in `docs/private-beach/tile-sizing-issues/README.md`. Verify why tiles still expand to 100% width and fix the CSS/layout logic so unlocked tiles default to ~400px width. Confirm that persisted layouts retain the corrected width. Next, investigate the terminal preview gaps caused by zoomed font sizing — make sure the preview matches the host output without missing border characters. Share logs/screenshots in `temp/` if helpful, add regression coverage if feasible, and provide a testable solution with instructions for validation.
```

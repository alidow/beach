# Private Beach Terminal Tile – Off-Screen Preview Rescue Plan (October 2025)

## 1. Situation Summary

The Private Beach dashboard renders live terminal previews in a 128-column grid whose tiles are capped around 450 px × 450 px. Hosts routinely advertise PTY sizes near 104×62, so the viewer must downscale each terminal into that bounding box. Today we clamp the BeachTerminal viewport to ~24 rows to keep the measured DOM height below the cap. That keeps the layout stable, but it crops the terminal (we either see only the top or the bottom depending on follow-tail).

## 2. Timeline & Experiments

| Phase | Change | Outcome |
| --- | --- | --- |
| Initial autosize | Grid layout migrated to 128 columns, ROW_HEIGHT=12; `TileCanvas` computes zoom from measurements vs. host size | Tiles respect new grid but PTY still renders at full height, so tiles stretch vertically |
| “Min(host, viewport)” clamp | `SessionTerminalPreviewClient` and `TileCanvas` switched to using `min(host, viewport)` for sizing | Zoom stabilised, but the DOM still measured at 62 rows, so tiles ballooned |
| Viewport clamp to 24 rows | Forced `forcedViewportRows=24`; autosize now succeeds | Visual crops return (only 24 rows rendered); follow-tail drags the window to the bottom |
| Follow-tail suppression | Attempted to disable follow-tail and pin viewport to top | Cache re-enabled follow-tail on new frames; crop persists |
| Background staging attempt | Added preview-status overlay, but still requested 24-row framebuffer | Layout stable but terminal still cropped to 24 rows |

Instrumentation (`temp/private-beach.log`) confirms the viewer reports `hostRows: 62`, but we keep rendering only `forcedViewportRows: 24`. The DOM height remains ~750 px before the CSS transform, so the tile either grows to match or we chop the viewport.

## 3. Current Problem Statement

We need to display the *entire* PTY (e.g. 104×62) inside a fixed ~450 px tile **without** unmounting the viewer, without reintroducing follow-tail loops, and without letting the DOM measurements grow past the cap. The present approach (clamping the viewport height) fundamentally conflicts with that requirement: the framebuffer we hand to the browser is actually only 24 rows tall.

## 4. Root Cause

React Grid measures the raw `offsetHeight` of our tile content. BeachTerminal renders each row at the natural line height (≈20 px), so 62 rows yield 62 × 20 px ≈ 1 240 px before we apply any CSS transforms. Even if we visually shrink the terminal via `transform: scale`, layout still sees the unscaled height. Our current workaround (clamp to 24 rows) keeps the grid happy but discards the rest of the PTY.

## 5. Proposed Off-Screen Staging Plan

1. **Stage the real BeachTerminal off-screen.** Mount it in an invisible wrapper (`position:absolute; width:0; height:0; overflow:hidden; opacity:0`). Let it connect immediately so we capture `hostViewportRows`, `hostCols`, latency, status, etc. The grid never measures this wrapper.

2. **Compute the downscale factor.** Once we know `hostCols`/`hostRows`, calculate the raw pixel size (`estimateHostPixelSize`) and derive `scale = min(MAX_TILE_WIDTH / width, MAX_TILE_HEIGHT / height)`. Keep the true aspect ratio; this yields `targetWidth`/`targetHeight` ≤ 450 px.

3. **Render a visible clone with fixed measurements.** Place a second wrapper in the tile with `width = targetWidth` and `height = targetHeight`. Inside, render BeachTerminal content at its natural size but wrap it in a child `<div>` with `transform: scale(scale)` and `transform-origin: top left`. Because the outer wrapper owns the scaled width/height, `offsetHeight` stays within 450 px even though we draw all 62 rows.

4. **Pin the viewport to the top.** After the PTY is loaded, call `store.setFollowTail(false)` and `store.setViewport(baseRow, hostRows)` (and subscribe to reapply) so the full buffer is always rendered from row 0 down. That prevents the cache from sliding the window to the tail.

5. **Measure & persist the scaled size.** Feed `targetWidth/Height` back into `TileCanvas` measurements and zoom maths so autosize uses the scaled dimensions instead of the raw PTY height.

6. **User experience:** While the off-screen viewer warms up, show a status overlay (“Connecting…”, “Preparing preview…”) in the tile. Reveal the visible clone only when the PTY size is known and the viewport is pinned. The user never sees stretched or cropped output during load.

## 6. Implementation Outline

1. **`SessionTerminalPreviewClient`:**
   - Split into two layers: a hidden “driver” BeachTerminal and a visible “preview” clone.
   - Track `hostCols/hostRows`, computed `scale`, `targetWidth/Height`, and preview status.
   - Render the visible clone with the fixed-size wrapper described above.
   - Emit `onPreviewReady` (with dimensions and scale) to the tile once the PTY metadata is available.
   - Keep the driver subscribed to the store to reapply `setFollowTail(false)` and `setViewport(baseRow, hostRows)` whenever frames arrive.

2. **`TileCanvas` / `SessionTile`:**
   - Store `previewMeasurements` (scaled width/height) alongside host dimensions and preview status.
   - Use those measurements when autosizing, snap-to-host, and zoom calculations run.
   - Display a loading overlay based on preview status until the scaled dimensions arrive.

3. **Remove the 24-row clamp:**
   - Delete `forcedViewportRows` from the visible clone (still pass it to the hidden driver if needed for measurements).
   - Ensure autosize no longer relies on `state.viewportRows` to cap DOM height; it should use the scaled measurements coming back from the preview.

4. **Diagnostics:**
   - Keep logging `render-props`, `dom-dimensions`, `viewport-clamped`, etc., to validate that host rows stay at 62 while the reported wrapper height matches the scaled target (~276 px).

## 7. Next Steps / Open Questions

1. **Implementation:** Build the “driver + clone” rendering model and plumb the scaled measurements through `SessionTile` and `TileCanvas`.
2. **Testing:** Verify no reconnect loops reappear (tiles shouldn’t unmount). Regression-test zoom/snapping and the lock/unlock workflow.
3. **UX polish:** Consider animating the transition from the loading overlay to the live preview once the scaled dimensions arrive.
4. **Optional:** Explore using `OffscreenCanvas` or a dedicated `<canvas>` renderer later for even smoother scaling, but the DOM/CSS approach above should unblock us now.

With these changes, BeachTerminal continues to render the full PTY while the dashboard tiles stay within the 450 px cap—no more cropped previews, no more giant tiles.

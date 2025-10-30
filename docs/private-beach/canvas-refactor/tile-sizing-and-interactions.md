# Private Beach Canvas — Tile Sizing, Drag/Drop, and Grouping Research

_Last reviewed: 2025-06-09 (Codex, post-review). See appended log at end of file for future updates._

## 1. Purpose
- Consolidate the long-running tile sizing issues (see `docs/private-beach/tile-sizing-issues/`) and translate them into requirements for the free-form canvas effort.
- Specify how tiles should be measured, scaled, and persisted when we move from `react-grid-layout` to a React Flow based canvas.
- Detail drag-and-drop behaviours (tile ↔ agent, tile ↔ tile, group moves) and their interactions with sizing/positioning.
- Surface implementation constraints, telemetry hooks, and open questions that must be resolved before build-out.

## 2. Legacy Insights Recap
- **Measurement drift:** RGL tiles often measured at full container width (~1500 px) before grid metadata was available, leading to zoom miscalculations and oversized layouts (`TileCanvas.handleMeasure` logs confirm this).
- **Terminal cropping vs. inflation:** Forcing the terminal viewport to 24 rows avoided layout blow-ups but cropped meaningful content. Rendering the full PTY in-place caused the DOM height to explode (>1 200 px) before transforms applied.
- **Off-screen staging plan:** `docs/private-beach/tile-sizing-issues/offscreen-preview-plan.md` recommends rendering the true terminal off-screen, computing a scale factor, and drawing a downscaled clone inside a fixed wrapper (<450 px). That approach decouples DOM measurements from PTY size.
- **Grouping gaps:** Current grid-based UI has no native grouping; controller assignments happen via side panels, and drag collisions reflow instead of overlapping.

## 3. Canvas Requirements
- Tiles must render arbitrary content (terminal, browser, stream) inside a target bounding box (default 448×448 px) without DOM inflation or cropping.
- Users can pan/zoom the canvas independently of tile-level scaling; zooming the viewport must not affect the intrinsic size we persist for tiles.
- Tiles can be resized manually (future) or auto-sized from content; the system must respect locked states.
- Dragging a tile onto another tile creates/extends a group; dragging onto an agent assigns control.
- Groups move as cohesive units; drop zones accept either individual tiles or groups.
- Tiles/agents/groups carry metadata (status, host size) without forcing re-layout.
- Canvas interactions must remain responsive with 50+ nodes; measurement operations should be O(n).

## 4. Tile Sizing Strategy (Free-Form Canvas)

### 4.1 Measurement Pipeline
1. **Driver mount (off-screen):** When a tile mounts, spin up the live preview inside an off-screen container (`position:absolute; pointer-events:none; opacity:0; width:0; height:0`). This instance negotiates WebRTC, receives PTY frames, and exposes accurate `hostCols/hostRows`. Set `disableViewportMeasurements` and apply `contain: size` so driver DOM never contributes to layout observers.
2. **Snapshot metrics:** Once metadata arrives, compute the unscaled host pixel dimensions using the existing helper (`estimateHostSize` equivalent) but store the raw values for reference.
3. **Scale calculation:**
   ```ts
   const MAX_TILE_WIDTH = 448;
   const MAX_TILE_HEIGHT = 448;
   const scale = Math.min(
     MAX_TILE_WIDTH / hostPixelWidth,
     MAX_TILE_HEIGHT / hostPixelHeight,
     1, // never upscale
   );
   const targetWidth = Math.round(hostPixelWidth * scale);
   const targetHeight = Math.round(hostPixelHeight * scale);
   ```
   - Allow an override for tiles that are manually resized or locked; in that case, use the explicit size instead of the max bounds.
4. **Visible preview:** Render a visible clone inside a wrapper with fixed dimensions (`width: targetWidth; height: targetHeight`). Apply `transform: scale(scale)` to the live preview content. Because the wrapper owns the scaled dimensions, React Flow’s node bounding box equals `targetWidth/Height`.
5. **State propagation:** Emit a `previewReady` event carrying `{ targetWidth, targetHeight, scale, hostCols, hostRows, measuredAt }`. The canvas state stores this as `tile.preview.measurements`.
6. **Persistence:** Persist `widthPx/heightPx` in the new `CanvasLayout` schema using the `targetWidth/Height`. Include `contentScale` (for diagnostics) but treat it as derived.
7. **Host resize API:** When a tile is locked or a user requests host fit, call `requestHostResize({ rows, cols })` with values derived from `targetWidth/Height` and host metadata. Debounce to avoid bursts while the host is already resizing.

### 4.2 Runtime Adjustments
- **Host resizing:** If the host announces new PTY dimensions, recompute the scale, bump a `measurementVersion`, and smooth the transition (CSS ~120 ms).
- **Manual resize:** When a user resizes a tile (future feature), update an explicit `manualSize` override. Downscale content proportionally unless locked.
- **Locked tiles:** Keep the last persisted `targetWidth/Height`; on host changes, issue `requestHostResize` with locked dimensions instead of recomputing scale.
- **Viewport zoom:** Canvas-level zoom multiplies all node positions/sizes visually but does not mutate `targetWidth/Height`. Persist zoom separately (`viewport.zoom`).

### 4.3 Accessibility & Performance
- For screen readers, expose the unscaled dimensions via ARIA descriptions (“Preview scaled to 320×180 from host 104×62”). Keep DOM nodes limited by using `transform: scale` instead of re-rendering at new sizes.
- Batch measurement updates using `requestAnimationFrame` to avoid thrashing React Flow layout. Only emit persistence updates when the user stops dragging or when the scale changes by >2%.

## 5. Drag-and-Drop Behaviour

### 5.1 Tile ↔ Canvas (Positioning)
- Dragging updates `position: { x, y }` in canvas coordinates (screen pixels at zoom 1). React Flow manages the transform; we debounce persistence (e.g., 200 ms after drop).
- Snap lines (optional): show alignment guides when close to multiples of 16 px; do not force snapping.
- When dragging a tile with an active controller, display a tether line to the agent node to maintain context.

### 5.2 Tile ↔ Agent (Assignment)
- On hover over an agent node, highlight the node and show drop affordance (“Assign Agent to Session”).
- Dropping triggers controller pairing logic:
  - For individual tiles: call `createControllerPairing` with `(sessionId, agentId)`.
  - For groups: call once per member tile. Consider queueing to avoid burst traffic; show aggregated progress in UI.
- Failure handling: revert visual assignment, toast error, keep tile selected for retry.
- Success: update `controlAssignments` in canvas state, add inline badge on agent tile showing controlled sessions.

### 5.3 Tile ↔ Tile (Grouping)
- Drop detection uses bounding boxes: if drop center lies within another tile’s hit area, prompt grouping.
- Flow:
  1. Source tile dropped on target tile.
  2. If neither is grouped ➜ create new `GroupNode` with both members, compute bounding box as union of child rectangles plus padding (e.g., 16 px).
  3. If target already in group ➜ append source to that group; recompute bounding box.
  4. If source belongs to different group ➜ confirm merge via quick modal (future) or move tile into target group directly (initial behaviour).
- Group visual: container box with subtle border, title bar for name (optional), stacked offset for child tiles. Child tiles maintain their own sizes; group holds relative offsets.
- Dragging group moves entire container; React Flow supported by treating group as parent node with `extent="parent"`.

### 5.4 Ungrouping & Reparenting
- Provide context menu (`right-click` or toolbar) with `Ungroup` and `Remove from Group`.
- When removing a tile from a group, preserve its absolute canvas position by translating from group-relative coordinates back to canvas coordinates.
- If group becomes single tile after removal, auto-dissolve group.

## 6. Group Geometry & Z-Order
- Group bounding boxes recompute on each child move:
  ```ts
  const padding = 24;
  const bounds = children.reduce(
    (acc, child) => ({
      minX: Math.min(acc.minX, child.position.x),
      minY: Math.min(acc.minY, child.position.y),
      maxX: Math.max(acc.maxX, child.position.x + child.size.width),
      maxY: Math.max(acc.maxY, child.position.y + child.size.height),
    }),
    { minX: +∞, minY: +∞, maxX: -∞, maxY: -∞ },
  );
  group.size.width = (bounds.maxX - bounds.minX) + padding * 2;
  group.size.height = (bounds.maxY - bounds.minY) + padding * 2;
  ```
- Child positions stored relative to group origin (`child.position = { x: absoluteX - group.minX - padding, y: ... }`).
- Z-order rules:
  - Default agent nodes below tiles.
  - Maintain `zIndex` per node; bumps to front on drag start.
  - Group container’s z-index always ≤ child tiles to ensure border encloses them.

## 7. Persistence Hooks
- Extend `CanvasLayout` (see `canvas-refactor-plan.md`) with:
  ```ts
  tiles[id]: {
    size: { width: number; height: number };
    contentScale: number; // derived but persisted for diagnostics
    hostDimensions: { cols: number; rows: number } | null;
    autoSize: boolean; // true unless user override
  }
  groups[id]: {
    memberIds: string[];
    padding: number;
  }
  ```
- Persist `dragHistory` (optional) to assist undo/redo later.
- Save operations triggered by:
  - Tile move/resize drop.
  - Group membership change.
  - Agent assignment success (after API confirms).
- Use ETags or `updatedAt` to avoid overwriting other clients’ changes (future multi-user support).

## 8. Telemetry & Diagnostics
- **Canvas events:** `canvas.tile.move`, `canvas.tile.resize`, `canvas.tile.group.create`, `canvas.tile.group.add`, `canvas.tile.group.remove`, `canvas.agent.assignAttempt`, `canvas.agent.assignSuccess/Failure`.
- **Sizing metrics:** log `targetWidth/Height`, `scale`, `hostRows/Cols` at preview ready to `console.info('[canvas-tile] size', …)` behind debug flag.
- **Error tracing:** capture exceptions during measurement or React Flow node updates; surface via Sentry with node ID and operation.
- **Performance budget:** instrument drag FPS (via `performance.now()` sampling) for canvases with 50 nodes; target >45 fps.

## 9. Accessibility Considerations
- Provide keyboard shortcuts for move (arrow keys ±10 px, shift for ±50 px) and grouping (e.g., `Cmd+G` to group selected tiles).
- Announce drop outcomes via `aria-live` (“Assigned Agent ‘Halcyon’ to Session ‘Prod CLI’”).
- Ensure focus stays within group when cycling through members; group container acts as landmark.
- Maintain high-contrast outlines for selected nodes; ensure border thickness scales with zoom (via CSS transform compensation).

## 10. Open Questions
1. **Viewport persistence vs. user preference:** Should we store distinct zoom levels per user or per beach? Need product decision.
2. **Tile resize handles:** Are manual resize controls in scope for v1 of the canvas? Impacts sizing persistence and UX complexity.
3. **Real-time collaboration:** If two users edit simultaneously, how do we reconcile tile positions? Out of scope now but influences data model (etag vs. CRDT).
4. **Snap-to-grid option:** Do power users still want optional snapping? Could provide toggle without reintroducing RGL constraints.
5. **Preview throttling thresholds:** Finalize FPS budgets (for example, 15fps for background tiles) and confirm UX impact.

## 11. Recommended Next Steps
1. Implement the refactored SessionTerminal preview (driver + clone + `requestHostResize`) directly in the new canvas codepath.
2. Define React Flow node types with size/position props influenced by the measurement state described above.
3. Work with backend to finalize layout v3 schema additions (size, scale, group padding) and the batch controller assignment endpoint.
4. Align with design on grouping visuals, drop affordances, and keyboard interaction patterns.

---

### Appendix A — Research Log

- **2025-06-09 (Codex):** Consolidated legacy tile sizing issues, proposed off-screen measurement strategy for canvas, defined drag/drop/grouping behaviours, and highlighted open backend questions.
- **2025-06-09 (Codex):** Incorporated critical review feedback—locked in explicit `requestHostResize`, added measurement versioning, preview throttling, and aligned sizing plan with aggressive greenfield canvas rollout.

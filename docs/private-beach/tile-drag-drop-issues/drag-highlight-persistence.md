# Canvas Tile Drag Highlight Persistence

## Summary
- After completing a drag operation in the React Flow-based canvas, the tile remains visually highlighted (border/glow) even though the drag has ended.
- The highlight persists across additional interactions until another node is selected or the canvas is reloaded.

## Environment
- Frontend: `apps/private-beach` (Next.js, React 18, React Flow 11)
- Recent canvas refactor work (SessionTerminalPreview integration, telemetry, drag fixes)
- Issue observed in dark theme with updated mini-map styling (Oct 30 build).

## Reproduction Steps
1. Load a beach dashboard that renders the new `CanvasSurface`.
2. Drag any application tile to a new position and release the mouse.
3. Observe the tile styling after the drop completes.

## Observed Behaviour
- The tile retains the active-selection styling (border+glow) and appears “stuck” in a dragged state even though the interaction finished.
- The same highlight shows in user-provided screenshot (`highlight-persistence.png`, see Slack thread dated Oct 30).

## Expected Behaviour
- Once the drag ends the tile should either:
  - Revert to the default (non-selected) styling, or
  - Stay selected only if React Flow marks it as selected, but without the transient drag glow.

## Current Implementation Notes
- While dragging we update the layout via `updateLayout` inside `handleNodeDrag`, so React Flow nodes mirror the cursor.
- On drop (`handleNodeDragStop`) we persist the layout and emit telemetry; we do not currently clear selection state.
- Tile styling derives from the `selected` flag in `TileNodeComponent` (border class) – we recently attempted to soften the styling but the highlight remains.
- React Flow may keep the node selected unless we explicitly clear it via `setSelection([])` after drop.

## Hypothesis / Next Steps
1. Investigate whether React Flow still marks the node as selected after the drop. If yes, consider calling `setSelection([])` or `setSelection([node.id])` conditionally.
2. Confirm whether the highlight class is coming from our Tailwind overrides or React Flow’s `.react-flow__node.dragging` styles.
3. Inspect computed styles in DevTools after drop to see which class remains applied (`.selected`, `.dragging`, etc.).
4. Update `handleNodeDragStop` to clear drag-only CSS classes (e.g., by resetting layout before React Flow re-renders).
5. Once fixed, add a regression test (Playwright or Cypress) to ensure tile returns to default styling post-drop.

## Hand-off Notes
- Code touchpoints: `apps/private-beach/src/components/CanvasSurface.tsx` (`handleNodeDrag`, `handleNodeDragStop`, `TileNodeComponent` styling).
- Telemetry currently emits `canvas.drag.stop` on drop; keep the event if adjusting selection behaviour.
- Please attach any additional screenshots or console output to this doc after further investigation.

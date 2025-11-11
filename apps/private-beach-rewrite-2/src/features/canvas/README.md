Canvas Dragging Do’s and Don’ts
===============================

This directory hosts the React Flow canvas and associated UI.

Do
- Use `nodeDragHandle=".rf-drag-handle"` on `<ReactFlow>` and ensure the node header has `rf-drag-handle`.
- Keep node props in sync during drag via `onNodesChange` + `setTilePositionImmediate`.
- Snap and commit on `onNodeDragStop`.
- Keep these props set: `onlyRenderVisibleElements={false}`, `elevateNodesOnSelect={false}`, `selectNodesOnDrag={false}`, and add `translateZ(0)` + `willChange` to the ReactFlow surface style.
- Disable auto-persistence while dragging (see `useTileLayoutPersistence({ auto: false })`). Persist on placement/drop.

Don’t
- Don’t disable `nodesDraggable` conditionally.
- Don’t update positions in `onNodeDrag`.
- Don’t animate transforms (avoid `transition: all`).
- Don’t keep blur/shadow/filters active during drag.

References
- FlowCanvas.tsx contains inline comments near `onNodesChange`, `<ReactFlow>` props, and pan/drag behavior.
- TileFlowNode.tsx contains inline comments describing compositor hints used during drag.


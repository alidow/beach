Private Beach Rewrite 2 – Canvas Dragging Guide
================================================

This package renders a React Flow–based canvas for draggable application/agent tiles.
Dragging stability depends on a few specific patterns. Please follow these to avoid regressions.

Key Principles
- Start drags only from the tile header using `nodeDragHandle=".rf-drag-handle"`.
- Keep node positions in sync during drag via `onNodesChange` with `setTilePositionImmediate`.
- Commit snapped position only on `onNodeDragStop`.
- Avoid heavy visual effects (blur/shadow/filter) during drag; restore them when idle.
- Do not schedule layout persistence while dragging; persist on placement and drop.

FlowCanvas (src/features/canvas/FlowCanvas.tsx)
- React Flow props to keep set:
  - `nodeDragHandle=".rf-drag-handle"`
  - `nodesDraggable`
  - `onlyRenderVisibleElements={false}`
  - `elevateNodesOnSelect={false}`
  - `selectNodesOnDrag={false}`
  - `panOnDrag` (pane-only; nodes use header handle)
  - `style={{ transform: 'translateZ(0)', willChange: 'transform' }}`
- Events:
  - `onNodesChange`: if `change.dragging` is true, call `setTilePositionImmediate(id, position)`.
    Otherwise, snap and call `setTilePosition(id, snapped)`.
  - `onNodeDragStop`: snap and commit final position; emit telemetry; request persistence.
- DO NOT write positions inside `onNodeDrag`.

TileFlowNode (src/features/tiles/components/TileFlowNode.tsx)
- The header element has class `rf-drag-handle`.
- While dragging:
  - Force compositor layer: `transform: translateZ(0)`, `willChange: 'transform'`, `backface-visibility: hidden`.
  - Reduce effects: disable filters/blur/shadow; pause transitions.
  - Temporarily disable pointer events inside the body to avoid hover churn.
- Keep transitions limited to non-transform properties.

Global CSS (src/app/globals.css)
- Contains a `@layer components` rule for `.react-flow__node.dragging` to enforce compositor hints and disable heavy effects during drag.

Persistence (src/features/canvas/useTileLayoutPersistence.ts)
- Call the hook with `{ auto: false }` to avoid auto-debounce while dragging.
- Persist explicitly on tile placement and after drag stop via the returned `requestImmediatePersist()`.

Regression Tests
- `src/features/canvas/__tests__/FlowCanvas.reactflow-props.test.tsx` validates required React Flow props are set to anti-flicker values.

Common Pitfalls (do not)
- Don’t set `nodesDraggable={!something}`; keep it enabled and gate drag via `nodeDragHandle`.
- Don’t update positions in `onNodeDrag`.
- Don’t auto-persist layout during drag or log per-move scheduling.
- Don’t animate transforms (no `transition: all`) on nodes.


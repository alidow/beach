AGENTS Guidance — private-beach-rewrite-2
=========================================

Scope
- Applies to everything under `apps/private-beach-rewrite-2/`.

Context
- This app renders a React Flow–based canvas with draggable tiles (applications/agents).
- We’ve hardened drag behavior to avoid flicker/disappearing nodes. Please keep the invariants below.

Non‑negotiable invariants (dragging)
- Start drags from the header only:
  - `<ReactFlow nodeDragHandle=".rf-drag-handle" />`
  - The node header element must have class `rf-drag-handle`.
- Keep `nodesDraggable` enabled (do not make it conditional). Gate the start area via `nodeDragHandle`.
- Keep external state in sync during drag:
  - In `onNodesChange`: if `change.dragging` is true, call `setTilePositionImmediate(id, change.position)` and return.
  - When not dragging, snap and `setTilePosition(id, snapped)`.
  - Do not write positions in `onNodeDrag`.
- React Flow props that must remain set:
  - `onlyRenderVisibleElements={false}` (prevents culling pop‑in during fast drags)
  - `elevateNodesOnSelect={false}` (prevents z‑index jumps while dragging)
  - `selectNodesOnDrag={false}`
  - `panOnDrag` (pane panning; node drags use the header handle)
  - Add `style={{ transform: 'translateZ(0)', willChange: 'transform' }}` on `<ReactFlow>`
- While dragging, reduce heavy visuals on nodes to avoid GPU repaint churn:
  - In `TileFlowNode`, during drag:
    - Force a compositor layer: `translateZ(0)`, `will-change: transform`, `backface-visibility: hidden`, `transform-style: preserve-3d`, `contain: layout paint`.
    - Disable filters/blur/shadows and pause transitions.
    - Temporarily disable pointer events inside the body.
  - Global CSS adds rules for `.react-flow__node.dragging` to enforce the same, including forcing `visibility: visible` / `opacity: 1` because React Flow injects `visibility: hidden` on the live node while cloning a drag ghost. Removing that override brings the blink back.

Persistence rules
- Do not schedule layout persistence during dragging.
- Use `useTileLayoutPersistence({ auto: false })` and call the returned `requestImmediatePersist()`:
  - After tile placement
  - After drag stop (when final position is committed)

Why these rules exist
- React Flow moves nodes via CSS transforms during drag. If our props lag behind, React Flow can reconcile to stale positions and cause flicker/blink.
- Filters/blur/shadows during per‑frame transforms can trigger expensive repaints or layer thrash.
- Debounced persistence/logging during fast drags adds main‑thread pressure that manifests as jank/visibility glitches.

File references
- FlowCanvas: `src/features/canvas/FlowCanvas.tsx`
  - Comments near `onNodesChange` explain why we call `setTilePositionImmediate` during drag.
  - `<ReactFlow>` props are annotated with rationale.
- Tile node: `src/features/tiles/components/TileFlowNode.tsx`
  - Comments describe compositor hints and visual reductions during drag.
- Global CSS: `src/app/globals.css`
  - Contains `.react-flow__node.dragging` overrides to stabilize node rendering.
- Persistence hook: `src/features/canvas/useTileLayoutPersistence.ts`
  - JSDoc explains using `{ auto: false }` and persisting on drop.

Tests (regression guard)
- `src/features/canvas/__tests__/FlowCanvas.reactflow-props.test.tsx` asserts the anti‑flicker/drag props are set. Update the test if you intentionally change those props; otherwise, keep them as is.

Quick checklist before you change canvas/drag logic
- [ ] `nodeDragHandle=".rf-drag-handle"` and header carries `rf-drag-handle`
- [ ] `nodesDraggable` is enabled (not conditional)
- [ ] `onNodesChange` updates with `setTilePositionImmediate` when `change.dragging`
- [ ] No position writes in `onNodeDrag`
- [ ] RF props: `onlyRenderVisibleElements=false`, `elevateNodesOnSelect=false`, `selectNodesOnDrag=false`, `panOnDrag`, RF surface has `translateZ(0)`
- [ ] Persistence disabled mid‑drag; persist on placement and drop
- [ ] No `transition: all` on nodes; avoid animating transforms
- [ ] Keep CSS overrides for `.react-flow__node.dragging`

If unsure
- Skim the comments in FlowCanvas/TileFlowNode and the README in this package.
- When in doubt, ask or preserve the current patterns — they are deliberate to prevent flicker/disappearing nodes under fast drags.

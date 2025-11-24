AGENTS Guidance — private-beach-rewrite-2
=========================================

Scope
- Applies to everything under `apps/private-beach-rewrite-2/`.
- `apps/private-beach` is **deprecated**; do not add new behavior there. All Private Beach UI/agent work should land in `apps/private-beach-rewrite-2` and supporting packages.

Context
- This app renders a React Flow–based canvas with draggable tiles (applications/agents).
- We’ve hardened drag behavior to avoid flicker/disappearing nodes. Please keep the invariants below.

Beach creation intent
- Creating a private beach must yield an empty canvas. No auto-applied templates/layouts (including Pong); any bootstrap is explicit opt-in (e.g., via a setup script or “load template” action).

Private Beach controller / agent integration
- Public sessions (e.g., CLI `beach host … host` players) never need to know about Private Beach credentials. They only point at:
  - `--session-server http://localhost:4132/` (Beach Road)
  - `PRIVATE_BEACH_MANAGER_URL=http://localhost:8080`
  - A join/attach code when prompted.
- After the session successfully attaches, Beach Manager dials the host itself, completes a handshake, and pushes an **auto‑attach hint** plus controller creds down the control channel:
  - Host logs show `manager handshake received; starting action consumer` followed by `auto-attach hint received via manager handshake` and `auto-attach via handshake … manager=http://127.0.0.1:8080 source="handshake"`.
-  The handshake is the *only* sanctioned way a host learns controller credentials. The hint contains a session-scoped controller token + attach metadata; hosts must **not** set `PB_MANAGER_TOKEN`, `PB_CONTROLLER_TOKEN`, or similar overrides for public sessions. If a host is missing controller access, fix the handshake/attach path rather than exporting long-lived tokens.
- Once the handshake finishes the host exposes a `mgr-actions` WebRTC data channel. Manager’s controller-forwarder binds to that channel and starts fast-path delivery; the host falls back to HTTP polling only if the fast-path never comes up.
- Keep an eye out for the following log sequence whenever things look stuck:
  1. `manager handshake received …`
  2. `auto-attach via handshake …`
  3. `controller action consumer starting …`
  If step 2 or 3 never appear, the host couldn’t accept the hint and the manager will eventually drop controller commands with `reason="missing_lease"`.
- The mock agent (`apps/private-beach/demo/pong/tools/run-agent.sh`) already obtains its own tokens via `beach login`; it does not reuse host creds. Running the agent does not require exporting host/session-specific secrets either.

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

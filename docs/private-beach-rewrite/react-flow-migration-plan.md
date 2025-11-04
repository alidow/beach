# Private Beach Rewrite · React Flow Migration Plan

## 1. Background & Goals

The current canvas stack inside `apps/private-beach-rewrite` uses a thin DnD Kit wrapper (`CanvasWorkspace`) plus a bespoke tile store to manage node placement, movement, and telemetry. This bespoke layer has been sufficient for prototyping, but it lacks several capabilities we need for the next phase:

- Rich node interactions (connecting nodes, zoom, pan, mini-map) without reimplementing them.
- Stable layout behaviour for large tile collections.
- Ecosystem support (plugins, components, type helpers).

**Goal:** replace the custom canvas renderer with [React Flow](https://reactflow.dev/) while preserving functional parity (placement, movement, telemetry) and creating a foundation for future features (edges, live state overlays).

## 2. Success Criteria

1. Canvas loads and renders existing tiles using React Flow nodes with no behavioural regressions.
2. Node placement via the catalog works, including grid snapping, telemetry events, and focus handling.
3. Dragging/resizing/moving tiles emits the same telemetry payloads and updates the store.
4. Unit/Playwright smoke tests exercise the new canvas and pass.
5. Flagged rollout: the rewrite canvas can toggle between the legacy renderer and React Flow until QA sign-off.

## 3. Architectural Overview

### Current State
- `CanvasWorkspace` orchestrates DnD Kit, telemetries tile moves, and renders `TileCanvas`.
- `TileCanvas` reads from the tile store (`useTileState`) and positions absolutely-positioned `TileNode` components inside a scroll container.
- Telemetry is emitted on placement (`canvas.tile.create`) and movement (`canvas.tile.move`).

### Target State
- React Flow hosts the canvas surface, nodes, and interactions.
- Each tile is a React Flow “node” with a custom `TileNode` renderer.
- The tile store becomes a thin adapter to/from React Flow state (`nodes` + metadata).
- DnD between the catalog and React Flow uses Flow’s `useReactFlow().addNodes`.

## 4. Migration Plan

### Phase 0 – Spike & Scaffolding
1. Install `reactflow` and types (`npm install reactflow @types/reactflow --save` inside rewrite app).
2. Create a thin `FlowCanvas` wrapper with a placeholder node to confirm rendering.
3. Verify styling: React Flow container should inherit the same surface look & feel as the current `TileCanvas`.

### Phase 1 – Node Rendering Parity
1. Implement `TileFlowNode` component that reuses existing `TileNode` UI.
2. Map tile store entries into React Flow `Node` objects (`id`, `position`, `data`).
3. Render nodes via `<ReactFlow nodes={mappedTiles} nodeTypes={{ tile: TileFlowNode }} />`.
4. Ensure viewport (fit to nodes) roughly matches current behaviour; disable zoom/pan initially if needed.

### Phase 2 – Drag & Placement
1. Replace DnD Kit drop handling with React Flow’s `useReactFlow().project` to convert screen coordinates.
2. On drag end, call `addNodes` with snapped coordinates (preserve grid-snapping logic).
3. Implement node dragging via Flow’s controlled `onNodesChange` to keep React Flow and tile store in sync.
4. Emit `canvas.tile.create` and `canvas.tile.move` telemetry within the Flow callbacks, mirroring current payloads.

### Phase 3 – Store Integration
1. Rework the tile store to either:
   - (Preferred) become a thin persistence layer that mirrors React Flow state, or
   - Wrap React Flow’s `useNodesState` and provide selectors for consumer components.
2. Ensure focus and active tile logic still works; Flow’s `onNodeClick` should dispatch focus updates.
3. Remove old absolute-positioned layout logic (`TileCanvas`, `.tile-canvas` styles no longer needed).

### Phase 4 – Controls & Enhancements
1. Add Flow UI extras (zoom, pan, mini-map) behind a feature flag.
2. Implement viewport persistence (store Flow viewport in local state or metadata API for restored sessions).
3. Expose Flow shortcuts (Ctrl/Command key handlers) for tile duplication, deletion if desired.

### Phase 5 – Cleanup & Rollout
1. Gate the new renderer behind an environment flag (`NEXT_PUBLIC_REWRITE_CANVAS_FLOW=1`). *(Update 2025-11-04: flag retired; React Flow is now always on.)*
2. Update Playwright tests to exercise both legacy and Flow paths (or update smoke tests after flipping flag).
3. Remove DnD Kit dependencies and deprecated components once Flow is the default.
4. Write a short migration guide for other teams (WS-D/E) showing how to wire new node types.

## 5. Implementation Checklist

- [ ] Install React Flow dependencies inside `apps/private-beach-rewrite`.
- [ ] Create `FlowCanvas` wrapper component.
- [ ] Port tile nodes to React Flow node renderer with existing UI.
- [ ] Implement placement + move logic using React Flow APIs.
- [ ] Sync telemetry, focus state, and tile store with Flow nodes.
- [ ] Remove obsolete components (`TileCanvas`, DnD Kit wrappers) once parity achieved.
- [ ] Update CSS (remove `.tile-canvas` styles; theme React Flow container).
- [ ] Add unit tests for placement helpers and viewport snapping.
- [ ] Update Playwright canvas smoke test to assert React Flow nodes render and respond to drag.
- [ ] Write migration notes in `docs/private-beach-rewrite/CHANGELOG.md`.

## 6. Testing Strategy

### Automated
- Unit tests for new adapter utilities (node mapping, coordinate projection).
- React Testing Library tests for `TileFlowNode` to ensure metadata renders (session status, controls).
- Update Playwright smoke (`private-beach-rewrite-smoke.pw.spec.ts`) to drag & drop via Flow selectors.

### Manual QA
- Place multiple tiles, confirm grid snapping.
- Drag/resizing (if supported) ensures telemetry logs still appear in dev console.
- Check behaviour at different viewport sizes (catalog overlay, scroll/zoom).
- Regression on tile interactions: focus, remove, reopen.

## 7. Roll-out Plan

1. **Feature Flag** *(retired 2025-11-04)*: Flow now ships enabled; staging rollout handled via rewrite flag only.
2. **QA Checklist**: coordinate with WS-D/E owners to verify Flow-based canvas meets parity.
3. **Enable in Production**: flip flag via environment variable once QA passes, monitor telemetry for anomalies.
4. **Cleanup**: remove legacy code path and flag after two release cycles.

## 8. Follow-up Enhancements

- Introduce edges (connections) for future orchestration features.
- Persist node layout using React Flow’s built-in serialization, integrate with manager API.
- Add mini-map, selection box, keyboard shortcuts for power users.
- Explore grouping and layering via Flow’s built-in node grouping.

---

**Maintainers**: WS-C Rewrite team  
**Primary files touched**: `apps/private-beach-rewrite/src/features/canvas/*`, `apps/private-beach-rewrite/src/features/tiles/*`  
**Target completion**: align with Milestone 4 of the rewrite roadmap.

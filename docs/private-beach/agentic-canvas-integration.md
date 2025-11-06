# Agent Tile Integration — apps/private-beach-rewrite-2

## Goal
Ship the agent tile/node type inside the actual rewrite dashboard (the `/beaches/[id]` page) so the workflow mirrors the production canvas while adding the UX we aligned on:
- Agent nodes live alongside regular session tiles (same catalog, persistence, and React Flow surface).
- Creating an agent happens inline on the tile (role/responsibility text fields, no side pane).
- Connections are drawn via the existing connector handles, with a capsule on the edge that collects the two questions (instructions + update cadence).
- Saved capsules collapse into pills that re-open in place for edits.

## Architecture Hooks
- `apps/private-beach-rewrite-2/src/features/tiles/store.tsx` — central tile state (positions, metadata). Needs new `nodeType: 'application' | 'agent'` plus agent-specific fields (`role`, `responsibility`, `capsules`).
- `apps/private-beach-rewrite-2/src/features/canvas/FlowCanvas.tsx` — React Flow renderer. We will extend `nodeTypes` and `edgeTypes` to include `AgentTileNode` + `AssignmentEdge` while keeping existing tile behavior.
- `TileFlowNode` currently renders the classic session tile. We will introduce `AgentTileFlowNode` (new component) and map based on `tile.nodeType`.
- Canvas events + persistence hooks (`useTileLayoutPersistence`) need to understand the agent metadata so layout exports/imports don’t drop fields. For now the data remains in client state; manager persistence will be wired later.

## Implementation Plan
1. **State model upgrades**
   - Extend `TileDescriptor` with `nodeType`, `agentMetadata`, and `relationships` arrays.
   - Actions: `UPSERT_TILE` will accept `nodeType`. New actions to toggle agent editing, save role/responsibility, and upsert relationship capsules per edge ID.
2. **Node components**
   - Add `AgentTileFlowNode` (mirrors sandbox `AgentNode`, adapted to the tile store + SessionTile chrome so it blends in).
   - Update `FlowCanvas` `nodeTypes` to include `agent` and dispatch connectors only when source is agent.
3. **Edges / capsules**
   - Introduce `AssignmentEdge` (adapted from `RelationshipEdge`). Use React Flow `edges` array that already exists for selection/resizing (currently empty). On connect, push an edge with `data` referencing tile IDs; store resulting instructions/mode back into the tile store for persistence.
   - Rendering: `FlowCanvas`’s `edgeTypes` includes `assignment`. The capsule uses store callbacks to update instructions/mode.
4. **User entry points**
   - Update tile catalog / add-tile menu so “Agent” is an option. On add, we create a tile with `nodeType: 'agent'`, blank metadata, and auto-open the inline editor.
   - Keep everything else identical (dragging/resizing, add session tile, etc.).
5. **Testing/Validation**
   - Manual: run rewrite-2 dev server (`npm run dev -- --port 3003`), visit `/beaches/<id>`, add an agent tile, fill role/responsibility, connect to an existing session tile, enter capsule answers, save, edit, remove.
   - Automated: run `npm run lint` (existing coverage). Optional: add a basic component test to ensure `AgentTileFlowNode` renders inline editor state.

## Follow-ups (out of scope now)
- Persist agent metadata via layout API + manager once contracts are ready.
- Wire capsules to controller pairing endpoints.
- Replace explorer UI to surface agent/application roles per the broader UX plan.

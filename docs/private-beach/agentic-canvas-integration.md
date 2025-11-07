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

## Implementation Plan (Updated)
1. **State model upgrades**
   - `TileDescriptor` includes `nodeType` and `agentMeta`. The tile store also owns a normalized `relationships` map (`relationshipOrder`) so edge state survives reloads.
   - Actions cover tile CRUD, agent editing toggles, and relationship add/update/remove. `ApplicationTile` continues to drive session metadata while agent metadata lives alongside the tile.
2. **Node components**
   - `TileFlowNode` renders either the existing application chrome or an agent wrapper (inline editor + live terminal preview + four-sided connector handles). Agent tiles always show the terminal preview even while editing metadata.
   - All tiles expose target handles; agent tiles also expose source handles.
3. **Edges / capsules / persistence**
   - React Flow edges derive from the store’s `relationships` map. Capsules open via `EdgeLabelRenderer` with an icon-only trigger; delete happens inside the form.
   - `tileStateToLayout` writes each tile’s `nodeType` + `agentMeta` into the layout payload (using `metadata.nodeType` and `metadata.agentMeta`) and serializes relationships into `metadata.agentRelationships`/`metadata.agentRelationshipOrder`. `layoutToTileState` hydrates both tiles and relationships from that payload.
4. **User entry points**
   - The node catalog includes “Agent Tile.” Dropping one opens the inline editor. Session attach + terminal preview uses the existing `ApplicationTile` component so no functionality diverges.
   - Dragging a connector prompts for instructions + cadence via the capsule UI; the data lands in the relationship store and persists with the layout.
5. **Testing/Validation**
   - Manual: run rewrite-2 dev server, add agents/applications, connect them, reload, confirm tile roles and connectors persist.
   - Automated: `npm run lint` (existing coverage). Optional targeted tests for layout serialization helpers.

## Follow-ups (out of scope now)
- Persist agent metadata via layout API + manager once contracts are ready.
- Wire capsules to controller pairing endpoints.
- Replace explorer UI to surface agent/application roles per the broader UX plan.

# Agentic Canvas Sandbox (apps/private-beach-rewrite-2)

## Purpose
- Prototype the agent/application node catalog and connector UX inside the Next.js rewrite (`apps/private-beach-rewrite-2`) without perturbing the shipping dashboard.
- Validate inline agent creation, connector-only assignments, and edge-mounted relationship capsules before wiring them to Beach Manager APIs.

## Scope
1. **Sandbox route** – new dev-only page at `/dev/agent-canvas` that mounts the experimental graph with mock data.
2. **Agent nodes** – draggable tiles with inline role/responsibility editors, assignment bar placeholders, and connector handles.
3. **Application nodes** – lightweight tiles we can target from connectors; seeded with demo names.
4. **Relationship capsules** – custom React Flow edge labels that collect/edit the minimal assignment metadata (instructions + update cadence options) and collapse into a clickable badge once saved.
5. **Local-only state** – all graph data lives in front-end state for now; no Beach Manager calls.

Out of scope: persistence, SSE wiring, explorer integration, or replacing the primary `/beaches/[id]` canvas.

## UX Snapshot
### Agent Creation (inline)
1. User clicks “Add Agent” (or drags from future catalog). The tile appears at an auto-offset with a two-field editor (Role, Responsibility).
2. User enters free-form text and clicks **Save** → card switches to read-only mode showing the entered text, ID badge, and connector handle.
3. Clicking **Edit** reopens the inline editor on that tile.

### Application Creation
- “Add Application” spawns a read-only tile (title + target handle). These act as drop targets for connectors.

### Linking via Connector
1. User grabs the handle on an agent tile and drags to a target (agent or application). Only connector handles are supported—no drag-and-drop of whole tiles for now.
2. When the connection lands, a custom capsule appears along the edge asking two questions:
   - “How should this agent manage the connected session?” (textarea)
   - “How should the agent receive updates?” (radio: _On idle summary_, _Managed session pushes via MCP_, _Poll every ___ seconds_)
3. User saves → capsule collapses into a pill showing the chosen update mode + truncated instructions. Clicking the pill reopens the editor; delete button removes the relationship.

## Architecture Notes
- Build a dedicated feature module (`src/features/agentic-canvas`) with:
  - `AgentCanvas` orchestrator (React Flow provider + toolbar).
  - Node components: `AgentNode`, `ApplicationNode`.
  - Edge component: `RelationshipEdge` (extends `BaseEdge` + `EdgeLabelRenderer`).
  - Local helpers/types for node/edge state and ID generation.
- State lives in `AgentCanvas` using `useState` and `useCallback`. Each node’s `data` carries callbacks so child components can request updates without context wiring.
- Initial seed: start with an empty canvas; buttons add nodes incrementally so we can test interactions.

## Implementation Steps
1. **Docs** – this plan.
2. **Feature scaffolding** – create `src/features/agentic-canvas/{types,AgentNode,ApplicationNode,RelationshipEdge,AgentCanvas}.tsx` plus `index.ts`.
3. **Sandbox page** – `src/app/dev/agent-canvas/page.tsx` that renders headline + instructions + `<AgentCanvas />` inside a constrained layout.
4. **State management**
   - Node creation helpers (incremental offsets, default sizes).
   - `onNodesChange`/`onEdgesChange` wrappers from React Flow for drag + selection behaviors.
   - `onConnect` guard to ensure source is an agent.
5. **Agent inline editor**
   - Render form when `data.isEditing` or role/responsibility empty.
   - Provide Save/Cancel (cancel deletes tile if it never had data).
6. **Relationship capsule**
   - Custom edge component that renders a form via `EdgeLabelRenderer` when `isEditing` is true; otherwise show summary pill with click → edit.
   - Form options limited to the three update modes described.
7. **Styling**
   - Use Tailwind classes already available in rewrite for consistency.
   - Provide visual states (hover outlines, handle colors, capsule shadows).
8. **Testing/manual verification**
   - `npm --prefix apps/private-beach-rewrite-2 run lint && npm run test -- agentic-canvas` (unit tests optional; at minimum ensure lint passes).
   - Manual QA steps documented in PR/notes.

## Manual QA Checklist
- Load `/dev/agent-canvas` → page renders instructions & empty canvas.
- Click “Add Agent” → inline fields appear. Fill + save → tile shows entered text.
- Add application tile → target-only handle visible.
- Drag connector from agent to application → capsule form opens. Enter instructions, choose each update mode option, and save → pill displays summary. Clicking pill reopens form.
- Create agent→agent edge; same behavior.
- Delete edge via pill button.
- Edit agent role/responsibility after initial save.

Once the sandbox behaves as expected we can iterate on persistence and eventually replace the main canvas.

# Private Beach Canvas — React Flow Surface Implementation

_Owner: Frontend Codex instance responsible for the new React Flow canvas. Log progress at the end._

## Objective
Stand up the React Flow–based `CanvasSurface` that replaces `TileCanvas`, rendering application tiles, agents, and groups with fully free-form positioning, zoom, and pan. Wire it to the new canvas layout contract and expose hooks for downstream features (grouping, assignments, measurement updates).

## Scope
- Project setup
  - Add React Flow dependencies, typings, and global styles (if needed).
  - Create the `CanvasSurface` entry point and swap it into `apps/private-beach/src/pages/beaches/[id]/index.tsx`.
- State management
  - Implement `useCanvasState` (likely Zustand or RTK) that holds the canvas graph, selection, transient drag state, undo buffer, etc.
  - Integrate with the backend APIs delivered by the “backend-canvas-contracts” track (fetch/save, optimistic updates).
  - Persist viewport zoom/pan per beach, keeping UI state separate from shared geometry when appropriate.
- Node rendering
  - Define node types for tiles, agents, groups, and future annotations (structures as per plan).
  - Ensure tile nodes accept measurement data from the terminal preview track without re-render storms (memoization, selectors).
  - Render overlays (toolbar, latency badges, drop affordances) without breaking drag/pan interactions.
- Interactions
  - Implement pan/zoom controls (gesture + buttons), keyboard shortcuts, and selection mechanics.
  - Provide hooks/events for grouping and agent assignment tracks (e.g., `onDropNode`, `onCreateGroup`, `onAssignAgent`).
  - Manage z-indexing and focus states across nodes/groups.
- Persistence & lifecycle
  - Debounce layout saves, track dirty state, and reconcile server responses.
  - Handle reconnects or stale data (e.g., when another editor changes the layout).
- Developer ergonomics
  - Add Storybook/preview environment if helpful (optional but encouraged).

## Out of Scope (Handled elsewhere)
- Terminal measurement pipeline (`terminal-preview-integration.md`).
- Backend schema/apis (`backend-canvas-contracts.md`).
- Detailed grouping/drag-drop mechanics and agent assignment logic (`grouping-and-assignments.md`) — coordinate on interfaces.
- Exhaustive testing frameworks (`testing-and-performance.md`) — but ensure unit hooks exist for testability.

## Interfaces & Coordination
- **Data contract**: consume the `CanvasLayout` graph from the backend track.
- **Terminal preview**: expose props/callbacks so the terminal track can push measurement updates into node state.
- **Grouping/assignments**: provide event hooks and allow injection of drop handlers; avoid baking hard-coded behaviours so parallel work can attach logic cleanly.
- **Testing track**: ensure key actions emit predictable events that they can target in integration tests.

## Deliverables Checklist
- [x] React Flow installed and configured.
- [x] `CanvasSurface` replaces `TileCanvas` in the beach dashboard.
- [x] Canvas state store implemented with load/save wiring.
- [x] Node components (tile, agent, group) built with memoization and styling.
- [x] Pan/zoom/selection interactions functional.
- [x] API integration complete with optimistic updates and error recovery.
- [ ] Developer docs (README snippet or in-code comments) describing architecture.

## Coordination Notes
- Touch the `apps/private-beach/src/pages/beaches/[id]/index.tsx` fetch logic carefully — other tracks depend on deterministic load/save hooks here.
- When adding new context providers, document the order so other contributors can register listeners without rework.

## Progress Log
_Append new entries; do not edit previous updates._

| Date (YYYY-MM-DD) | Initials | Update |
| ----------------- | -------- | ------ |
| 2025-10-30 | CAI | Initial scaffold landed: added `reactflow` dependency and global styles; created `CanvasSurface` with React Flow baseline (tile node type, pan/zoom, selection), lightweight canvas state/hooks (`src/canvas/*`) exposing `useCanvasState/useCanvasActions` and handler registration. Swapped page `beaches/[id]/index.tsx` to load `CanvasSurface` (SSR disabled) in place of `TileCanvas`. Persist currently adapts node moves into legacy grid `onLayoutPersist` to avoid breaking flows while backend v3 endpoints land. |
| 2025-10-30 | CAI | Drag persistence patched so we clone React Flow nodes with their final `position` before saving and pushing back into store; manual drag→refresh check confirms layout sticks. Marked the React Flow + swap deliverables as complete. |
| 2025-10-30 | CAI | Synced with backend (v3 canvas endpoints + batch assignments live) and grouping/terminal tracks. Next up: wire CanvasSurface to new `CanvasLayout` API, integrate grouping handlers + keyboard shortcuts, adopt shared measurement telemetry, and migrate perf instrumentation from `TileCanvas`. |
| 2025-10-30 | CAI | CanvasSurface now hydrates from manager `getCanvasLayout` and persists via `putCanvasLayout` with optimistic updates (drag + viewport pan). Beach page keeps legacy preset state but promotes new layout state, default-seeding missing tiles, and routes add/remove flows through v3 saves. |
| 2025-10-30 | CAI | Replaced local graph state with shared `canvas/*` modules: CanvasSurface now renders tile/agent/group nodes via `GroupNode`, syncs selection through the shared store, and pipes drag/drop → `previewDropTarget/applyDrop` (including pending assignments + handler callbacks). Keyboard shortcuts (Cmd/Ctrl+G, Shift+Cmd/Ctrl+G) call into grouping reducers. |
| 2025-10-30 | CAI | Integrated SessionTerminalPreview into CanvasSurface: measurement updates resize tiles with version checks, persist optimistically, and emit telemetry (`canvas.measurement`, `canvas.resize.stop`, `canvas.drag.*`, `canvas.layout.persist`). |

### Near-Term Implementation Plan
1. **Assignment + grouping integration:** hand pending assignments off to the backend helper (`fulfillPendingAssignment`), expose handler docs/examples, and align telemetry with backend confirmations.
2. **Developer docs:** document CanvasSurface architecture (store sync, measurement pipeline, handler APIs) so grouping/terminal/testing tracks can plug in without reverse-engineering.

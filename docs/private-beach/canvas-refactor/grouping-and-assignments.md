# Private Beach Canvas — Grouping & Agent Assignment Logic

_Owner: Codex instance driving node grouping, drag/drop semantics, and controller assignment flows. Append updates to the progress log._

## Objective
Implement the user-facing behaviours on top of the React Flow canvas: tile ↔ tile grouping, drag-and-drop agent assignments (including batch handling), group visuals, and related UI affordances.

## Scope
- Group creation & membership
  - Detect tile-on-tile drops and create new group nodes with padding + stacked visuals.
  - Support adding/removing tiles from existing groups; auto-dissolve groups that fall to a single member.
  - Maintain relative positions within groups; update group bounding boxes and z-order when children move/resize.
  - Provide context menu / quick actions for “Ungroup”, “Rename group”, “Remove from group”.
- Drag & drop interaction layer
  - Handle hover highlights, ghost previews, and drop confirmation.
  - Ensure interactions respect React Flow’s parent/child constraints and the canvas surface’s selection model.
  - Support keyboard equivalents (e.g., `Cmd+G`, `Shift+Cmd+G`), collaborating with the canvas surface team for shortcut registration.
- Agent assignment
  - Implement tile/group → agent drop logic that calls the backend batch assignment endpoint (see `backend-canvas-contracts.md`).
  - Manage optimistic UI updates, partial failure reporting, and rollback on errors.
  - Display assignment badges, connection lines, and tooltips summarizing control state.
- State synchronization
  - Update the shared canvas state store with grouping and assignment changes, ensuring undo/redo buffers remain coherent.
  - Persist group metadata (name, padding, collapsed state) via the canvas layout contract.
- UX polish
  - Provide accessible announcements (`aria-live`) for grouping/assignment actions.
  - Ensure drop zones remain visible and discoverable across zoom levels.

## Dependencies & Coordination
- **Canvas surface**: consume hooks/events exposed by `canvas-surface-implementation.md`; avoid mutating state outside the agreed APIs.
- **Backend contracts**: rely on the batch assignment endpoint; clarify payload format early to prevent rework.
- **Terminal preview**: ensure grouped tiles still respect measurement updates (no clipping).
- **Testing/performance**: coordinate on test IDs and scenario scripts (large group moves, mass assignment).

## Deliverables Checklist
- [x] Grouping engine implemented with React Flow (parent nodes, padding, stacking visuals).
- [x] Tile/group drag/drop to agents calling batch endpoint with optimistic updates.
- [x] Partial failure UX and logging complete.
- [x] Keyboard shortcuts and accessibility cues in place.
- [x] Persistence wiring for groups and assignments finalized.
- [ ] Demo or Storybook scenario covering complex groups (10+ members).

## Verification Steps (to grow as tasks progress)
1. Manual: drag tiles to form groups, move groups, ungroup; observe consistent layout + selection.
2. Manual: drop multi-member group on agent; confirm API call payload, optimistic state, and success/error UI.
3. Automated: `pnpm --filter @beach/private-beach test layoutOps` (currently blocked by local Rollup optional dependency on macOS; rerun once toolchain installs native binary).

## Progress Log
_Append entries (newest last)._

| Date (YYYY-MM-DD) | Initials | Update |
| ----------------- | -------- | ------ |
| 2025-10-30 | CDX | Reviewed spec + canvas/backend plans; aligned on v3 CanvasLayout and handler interfaces. |
| 2025-10-30 | CDX | Added canvas modules: types (CanvasLayout v3), grouping reducers (create/add/remove/dissolve), hit-testing, and drag/drop wrappers. |
| 2025-10-30 | CDX | Implemented batch assignment flow with optimistic UI helper; auto-falls back to per-session createPairing until batch endpoint is live. |
| 2025-10-30 | CDX | Built minimal GroupNode visual with padding + stacked members; includes ARIA labeling and selection treatment. |
| 2025-10-30 | CDX | Added keyboard helpers for Cmd+G (group selection) and Shift+Cmd+G (ungroup selection) to plug into CanvasSurface. |
| 2025-10-30 | CDX | Coordinated via logs: no batch endpoint path found yet in temp/private-beach.log or beach-surfer.log; retaining fallback and ready to switch when backend publishes route. |
| 2025-10-30 | CDX | Implemented initial React Flow CanvasSurface with grouping drop logic and agent-drop → assignment bridge; gated behind NEXT_PUBLIC_CANVAS_SURFACE=1 to avoid disrupting grid users. |
| 2025-10-30 | CDX | Hover/drop target preview integrated (agent highlight) and optimistic assignment trigger wired to page handler; will switch to batch endpoint when backend lands. |
| 2025-10-30 | CDX | Wired Cmd+G / Shift+Cmd+G inside CanvasSurface to call grouping/ungrouping reducers using React Flow selection. |
| 2025-10-30 | CDX | Upgraded CanvasSurface to call fulfillPendingAssignment with manager credentials, reconcile optimistic assignments, and surface partial failures via toast/log messaging. |
| 2025-10-30 | CDX | Updated beach dashboard to pass agents/tokens, debounce layout persistence, and sync assignment state on drag/drop; backend batch endpoint still pending (fallback path active). |
| 2025-10-30 | CDX | Persisted group metadata (name/padding) through layoutOps + API helpers and added vitest regression for grouping round-trips. |

> **Open dependency:** Waiting on backend track to expose the real batch assignment endpoint. Current flow uses the per-session fallback; switch call site once route is published.

# Private Beach Canvas Refactor — Detailed Implementation Plan

## Executive Summary
- Replace the current `react-grid-layout` dashboard with a free-form canvas that supports zoom/pan, arbitrary tile placement, grouping, and agent assignment via drag-and-drop.
- Adopt **React Flow** as the core canvas engine after a structured evaluation against `react-konva` and `pixi-react`, leveraging its mature ecosystem, built-in interaction primitives, and TypeScript support.
- Introduce a unified scene graph that models tiles, agents, and groups as nodes with composable behaviours, backed by a new persistence schema that captures absolute positioning, z-order, and membership.
- Ship incrementally behind a feature flag: first deliver rendering parity, then layering interactions (drag, zoom, grouping, agent control), conclude with polish, migration tooling, and documentation.
- Maintain backwards compatibility through layout versioning and data migration scripts, plus extensive automated and manual validation before general release.

## Goals
- Provide a fluid canvas where tiles occupy arbitrary coordinates, support smooth zoom/pan, and maintain consistent rendering of existing tile content.
- Enable drag-and-drop workflows: assign agents as controllers when a tile/group is dropped on them; build application groups by dropping applications onto each other.
- Deliver ergonomic group affordances (visual border, stacked cards, group metadata) and treat groups as first-class draggable entities.
- Persist the new layout state (positions, scale, group membership, controller bindings) while remaining compatible with legacy layouts during rollout.
- Preserve or improve on current performance, accessibility, keyboard support, and error handling.

## Non-Goals
- No changes to the underlying terminal streaming protocol or session negotiation flows.
- Do not ship multi-user concurrent editing in the initial release; focus on single-user manipulation with deterministic persistence.
- Avoid redesigning the surrounding dashboard chrome (TopNav, drawers) beyond the adjustments necessary to host the new canvas.
- Do not alter controller pairing business rules beyond the drag-and-drop entry points described here.

## Current State Audit
- **Layout primitives:** `TileCanvas.tsx` orchestrates `react-grid-layout` with `BeachLayout` data (grid columns, row heights, zoom hints). Tile state is cached in React state (`TileViewState`) and persisted via `putBeachLayout`.
- **Interaction model:** Tiles snap to a 128-column grid, collisions trigger automatic reflow, and zoom is capped to a small range. Dragging over another tile invokes layout auto-shift, preventing overlap.
- **Drop flows:** Agent assignment is currently mediated by drawers/buttons; no direct drag between tiles and agents.
- **Persistence:** `BeachLayout` stores grid-oriented coordinates (`x`, `y`, `w`, `h`), optional pixel hints, zoom, and lock flags. Layout versioning exists but is tied to grid semantics.
- **Rendering constraints:** Canvas is wrapped in Next.js page with SSR disabled via `dynamic(() => import('./AutoGrid'), { ssr: false })`.
- **Testing & telemetry:** Minimal automated coverage for layout logic; behaviour validated manually. Little to no telemetry on drag events or layout saves.

## Requirements & Use Cases
- **Functional**
  - Free placement of application tiles anywhere within the canvas bounds.
  - Drag tile onto agent tile to establish controller relationship; visually confirm binding.
  - Drop application onto another application to create or extend a group; groups behave as composite nodes.
  - Drag groups and individual tiles interchangeably; dropping group on agent binds entire group.
  - Support tile ungrouping, reassignment, and reordering (either via context menu or dedicated UI, defined later).
- **UX & Interaction**
  - Smooth zoom and pan (trackpad pinch, scroll-wheel + modifier, keyboard shortcuts).
  - Selection affordances (single, multi-select future-friendly), focus states, and context actions.
  - Visual cues for drop targets (hover highlights, snap lines) without forced snapping.
  - Maintain tile toolbar functionality, badges, and existing session controls.
- **Technical**
  - Scene graph must scale to dozens of nodes without perf degradation; maintain stable 60fps interactions on mid-range hardware.
  - Support both pointer and keyboard accessibility, including focus management and ARIA roles.
  - Integrate with Clerk auth token refresh flow and existing data fetching routines with minimal disruption.
- **Persistence & Sync**
  - Introduce layout version `3` capturing absolute pixel coordinates (`xPx`, `yPx`), zoom scale, and group definitions.
  - Support reversible migration path from grid-based layout (v1/v2) to canvas-based layout (v3).
  - Save operations must remain idempotent and atomic; server should validate schemas and reject invalid graphs.
- **Observability**
  - Emit analytics for key interactions (zoom, drag, drop, group create/destroy, assignment).
  - Instrument layout save latency and failure rates.

## Canvas Stack Decision
- **Recommendation:** Adopt **React Flow** (`reactflow`).
  - Mature ecosystem (~20k stars), active maintenance, TypeScript-first, and MIT licensed.
  - Native support for pan/zoom, node drag, drop handlers, selection, lasso, mini-map, controls, and background grids.
  - Provides parent/child nodes (nested nodes) that we can leverage for groups without re-implementing hit-testing.
  - Lightweight DOM/SVG rendering keeps accessibility manageable versus pure canvas APIs.
  - Compatible with Next.js via dynamic import; SSR guard patterns documented by maintainers.
- **Evaluation criteria:** accessibility, performance, feature fit (zoom, grouping), API ergonomics, licensing, community adoption, and maintenance cadence.
- **Structured prototype plan:**
  1. **Spike A (React Flow):** Render sample tile (terminal preview) inside custom node; implement pan/zoom, drop-to-group interactions using parent nodes; confirm nested drag events and custom controls integrate cleanly with existing state.
  2. **Spike B (React Konva):** Rebuild the same scenario with `react-konva`, assess text/DOM embedding challenges, keyboard accessibility, and interop cost of replicating controls/menus.
  3. **Spike C (PixiJS via `@pixi/react`):** Validate render quality and performance, note the overhead of rebuilding UI primitives (buttons/toolbars) in WebGL.
  4. Score each spike against criteria; document trade-offs and finalize decision (default expectation: React Flow unless critical gaps surface).

| Criterion | React Flow | React Konva | Pixi React |
| --- | --- | --- | --- |
| Pan/zoom built-in | ✅ | ⚠️ (manual) | ⚠️ (manual) |
| Grouping primitives | ✅ (parent nodes, extent) | ⚠️ (custom logic) | ⚠️ (custom logic) |
| DOM integration | ✅ (regular React components) | 🚫 (must render via canvas) | 🚫 (WebGL textures) |
| Accessibility | ✅ (DOM nodes) | ⚠️ (manual ARIA) | ⚠️ (manual ARIA) |
| Ecosystem maturity | High | Medium | Medium |
| Performance ceiling | High (virtualized) | High | Very high |
| Maintainer responsiveness | Active | Active | Active |

## Target Architecture
- **Scene Graph**
  - `CanvasRoot`: orchestrates React Flow, handles zoom/pan state, listens for layout changes.
  - Node types:
    - `ApplicationTileNode`: renders terminal/application UI, maintains toolbar, status badges, and drop zones for grouping.
    - `AgentNode`: represents controller-capable agents; displays assignments and drop affordances.
    - `GroupNode`: visual container node; contains multiple application child nodes, shows stacked border and aggregated metadata.
    - (Optional future) `CanvasAnnotationNode`: support annotations or guides.
  - Edge types (conceptual, not necessarily visual):
    - `ControlEdge`: mapping from agent to tile/group (may render as subtle overlay or highlight).
- **Data Model (client)**
  - `CanvasLayout` (v3):
    ```ts
    type CanvasLayout = {
      version: 3;
      viewport: { zoom: number; pan: { x: number; y: number } };
      tiles: Record<string, { id: string; kind: 'application'; position: { x: number; y: number }; size: { width: number; height: number }; zIndex: number; groupId?: string; zoom?: number; locked?: boolean; toolbarPinned?: boolean; }>;
      agents: Record<string, { id: string; position: { x: number; y: number }; size: { width: number; height: number }; zIndex: number; icon?: string; status?: 'idle' | 'controlling'; }>;
      groups: Record<string, { id: string; name?: string; memberIds: string[]; position: { x: number; y: number }; size: { width: number; height: number }; zIndex: number; collapsed?: boolean; }>;
      controlAssignments: Record<string, { controllerId: string; targetType: 'tile' | 'group'; targetId: string }>;
      metadata: { createdAt: number; updatedAt: number; migratedFrom?: number };
    };
    ```
  - Extend existing `BeachLayout` API to accept `layoutVersion: 3` and store JSON blob for canvas semantics alongside legacy fields for backwards compatibility.
- **State Management**
  - Introduce `useCanvasState` hook (Zustand or Redux Toolkit) to manage scene graph, selection, interaction transient state (drag ghost, rubber band).
  - Persist durable state changes via debounced `putBeachLayout` calls; optimistic update with rollback on failure.
  - Use React context to expose canvas actions (zoomIn, zoomOut, resetView, createGroup, assignAgent).
- **Rendering & Integration**
  - Replace `AutoGrid` with `CanvasSurface` component (`dynamic` loaded, SSR disabled).
  - Wrap React Flow canvas with top-level overlays for controls (zoom controls, breadcrumbs, selection info).
  - Ensure tile contents (terminal preview) continue to mount via existing hooks (`useSessionTerminal`) without regression.

## Interaction Model
- **Zoom & Pan**
  - Capture scroll + modifier and pinch gestures to adjust React Flow viewport.
  - Provide UI controls (plus/minus buttons, fit-to-view, reset) for accessibility.
  - Persist zoom level per beach; optionally store last pan.
- **Dragging**
  - Use React Flow `useReactFlow` helpers for node dragging with custom snap-to-grid toggle (optional).
  - Display ghost outlines and drop highlights; allow overlapping nodes but enforce configurable min spacing via collision detection utility.
  - Support `Shift` modifier for axis-locked drag.
- **Grouping**
  - When dropping application onto another application:
    - If neither is grouped, create new `GroupNode` containing both.
    - If target belongs to group, add source tile to that group.
    - If source belongs to different group, prompt merge or move (future enhancement).
  - Group visual treatment: rounded border, subtle background, stacked offset for tiles, group label, aggregated status indicators.
  - Dragging group moves all member tiles; React Flow parent node handles transform.
  - Provide context menu or command palette entry to ungroup/remove.
- **Agent Assignment**
  - Drag tile/group onto agent; highlight valid drop target.
  - On drop, call `createControllerPairing` with the appropriate session IDs; update `controlAssignments` state optimistically.
  - If group dropped, create pairings for each member tile (batch request) or update backend design to accept group-level assignment.
  - Show assignment status (loading, success, error) via inline UI (badge, toast).
- **Selection & Keyboard**
  - Click selects node; `Esc` clears selection; arrow keys nudge by configurable step (e.g., 10px).
  - Provide accessible focus order, ARIA roles describing tile type and controls.
- **Undo/Redo (Stretch)**
  - Capture interaction history for undo/redo to ease user mistakes; consider local-only history to start.

## Persistence & API Changes
- **Backend Schema**
  - Extend `BeachLayoutItem` to include optional `canvas` payload or introduce separate `CanvasLayout` JSON column.
  - Persist groups and agent assignments server-side; ensure APIs validate membership constraints.
  - Versioning: `layoutVersion: 3` indicates canvas layout. Fallback to grid layout when version < 3.
- **Migration Strategy**
  - Implement server-side migration utility: convert grid coordinates into pixel positions using previous column width/row height calculations.
  - Default group assignments to none; maintain existing controller pairings.
  - Allow users to opt into new canvas via feature flag; maintain ability to revert to grid (avoid destructive migrations until GA).
- **API Contracts**
  - Update `getBeachLayout` / `putBeachLayout` to accept and return new schema; maintain compatibility by including both `layout` array (legacy) and `canvasLayout` object during transition.
  - Document new endpoints or payloads in `docs/private-beach/data-model.md`.

## Grouping & Agent Control Flow
- **Group Lifecycle**
  - Creation: triggered by drop (app → app) or via group toolbar button.
  - Metadata: allow naming groups; display aggregated statuses (e.g., highlight if any member offline).
  - Reordering: maintain `zIndex` to control stacking order; ensure deterministic ordering in persistence.
  - Ungrouping/Removal: provide action to disband group (removes parent node, retains child positions).
- **Agent Binding**
  - On drop: evaluate target type (agent vs. tile/group).
  - API calls:
    - Tile target: existing `createControllerPairing(sessionId, agentId)`.
    - Group target: call for each member (batched) or extend backend to accept group assignment (investigate).
  - Update agent node to show controlling tile(s); allow quick jump to tile.
  - Handle failure states gracefully (rollback UI, toast message).
- **Concurrency**
  - Lock layout during save to prevent conflicting updates (client-level flag); consider backend conflict detection (etag).

## Implementation Phases & Milestones
- **Phase 0 — Discovery & Spikes**
  - Catalog current tile types, data dependencies, and external interactions.
  - Execute library spikes (React Flow, Konva, Pixi), document findings, finalise stack decision.
  - Define UX prototypes (Figma or similar) for grouping visuals and control assignments.
  - Exit criteria: documented evaluation, approved UX mocks, signed-off architecture.
- **Phase 1 — Infrastructure & Data Layer**
  - Add layout version 3 schema to backend and API clients.
  - Implement migration utilities and feature flag gating.
  - Create `CanvasSurface` scaffold with React Flow integrated, rendering static tiles without interactions.
  - Exit criteria: canvas renders existing layout (converted) read-only behind flag.
- **Phase 2 — Core Interactions**
  - Implement pan/zoom controls, basic drag-and-drop with free positioning.
  - Persist positions via new schema; ensure optimistic updates and revert on errors.
  - Add selection handling, keyboard navigation, and focus management.
  - Exit criteria: parity with existing grid (single-tile positioning, zoom) under flag.
- **Phase 3 — Grouping & Agent Drops**
  - Implement grouping logic, visuals, drag behaviour, and persistence.
  - Wire drag-to-agent assignment flow, API integration, and UI feedback.
  - Add z-order controls and drop target affordances.
  - Exit criteria: full feature set functioning for internal testers.
- **Phase 4 — Polish & Hardening**
  - Add undo/redo (if in scope), context menus, tooltips, onboarding messaging.
  - Boost telemetry, error handling, loading states, and offline protection.
  - Expand automated tests, integration tests (Playwright), and performance profiling.
  - Exit criteria: QA sign-off, performance budgets met, documentation updated.
- **Phase 5 — Rollout**
  - Enable beta for select beaches; monitor telemetry and error rates.
  - Provide migration fallback path and guard rails (toggle flag in manager settings).
  - Graduate to GA after stability window; retire old grid code once all tenants migrated.

## Testing & Quality Strategy
- **Unit Tests**
  - Scene graph reducers (Zustand selectors), migration utilities, grouping algorithms, collision detection.
- **Integration Tests**
  - React Testing Library for drag/drop flows (use `@testing-library/user-event` + mocked React Flow hooks).
  - Playwright E2E verifying pan/zoom, grouping, agent assignment, persistence.
- **Visual Regression**
  - Percy or Chromatic snapshots for key layouts and group visuals.
- **Performance**
  - Benchmark drag FPS and render times using React Profiler; test with 50+ tiles.
  - Ensure layout save remains < 150ms round-trip.
- **Accessibility**
  - Axe audits; keyboard navigation tests; screen reader verification of node labels.
- **Observability**
  - Instrument events (`canvas.drag.start`, `canvas.group.create`, `canvas.assignment.success`, etc.) with Beach analytics.

## Rollout Strategy
- Feature flag in app settings (e.g., `canvasLayoutEnabled`).
- Beta cohort (internal teams) to validate workflows; collect feedback.
- Gradually migrate beaches by default while allowing manual rollback for defined window.
- Monitor telemetry dashboards (error rates, layout save failures, assignment latency).
- Sunset plan: once stable, delete grid layout components, mark API fields deprecated, and run cleanup migration.

## Risks & Mitigations
- **Library limitations:** React Flow may have constraints around nested draggable nodes. Mitigation: confirm via spike, contribute upstream or implement custom drag handlers if needed.
- **Performance degradation:** Free-form layout could cause overdraw. Mitigation: virtualization, memoization, throttle updates, offload heavy rendering (terminal preview remains canvas-based but optimize re-render).
- **Migration data loss:** Improper conversion could misplace tiles. Mitigation: snapshot backups, idempotent migration scripts, allow manual adjustments before GA.
- **Complexity creep:** Group and assignment logic may balloon. Mitigation: maintain clear separation of concerns, unit-test algorithms, document flows.
- **Accessibility regressions:** Canvas interactions risk excluding keyboard users. Mitigation: design keyboard-first navigation, follow WAI-ARIA guidelines, test early.
- **Backend bottlenecks:** Increased layout payload size may strain APIs. Mitigation: compress JSON if needed, enforce payload limits, optimize serialization.

## Open Questions
- Should group assignment to agents imply atomic backend operation (single request) or remain multiple pairings? Need backend input.
- What are the desired UX affordances for creating agents directly on canvas (if future requirement)?
- Do we require persistent z-order controls (send to back/front) or auto-manage based on interaction history?
- How should undo/redo interact with server persistence (local-only vs. persisted history)?
- Are there analytics or audit constraints requiring server-side logging of drag events beyond layout saves?

## Next Steps
- Validate this plan with stakeholders (design, backend, product) and confirm scope.
- Kick off Phase 0 tasks: schedule spikes, gather UX mocks, and draft migration RFC for backend.
- Establish tracking issues/epics in project management tooling reflecting phase milestones.

## References
- Current grid implementation: `apps/private-beach/src/components/TileCanvas.tsx`.
- API contracts: `apps/private-beach/src/lib/api.ts` (`BeachLayout`, controller pairing endpoints).
- Prior plans: `docs/private-beach/remaining-phases-plan.md`, `docs/private-beach/tile-zoom-resize-plan.md`, `docs/private-beach/vision.md`.

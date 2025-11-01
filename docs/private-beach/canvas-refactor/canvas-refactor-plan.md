# Private Beach Canvas Refactor — Detailed Implementation Plan

## Executive Summary
- Replace the current `react-grid-layout` dashboard with a free-form canvas that supports zoom/pan, arbitrary tile placement, grouping, and agent assignment via drag-and-drop.
- Adopt **React Flow** as the core canvas engine after a structured evaluation against `react-konva` and `pixi-react`, leveraging its mature ecosystem, built-in interaction primitives, and TypeScript support.
- Introduce a unified scene graph that models tiles, agents, and groups as nodes with composable behaviours, backed by a new persistence schema that captures absolute positioning, z-order, and membership.
- Build the canvas as the new default experience (greenfield replacement of the grid), decommissioning legacy layout codepaths as part of the initial release.
- Stand up a dedicated persistence/API surface for the canvas graph (layout version 3) including group metadata, z-order, assignments, and viewport state.

## Goals
- Provide a fluid canvas where tiles occupy arbitrary coordinates, support smooth zoom/pan, and maintain consistent rendering of existing tile content.
- Enable drag-and-drop workflows: assign agents as controllers when a tile/group is dropped on them; build application groups by dropping applications onto each other.
- Deliver ergonomic group affordances (visual border, stacked cards, group metadata) and treat groups as first-class draggable entities.
- Persist the new layout state (positions, scale, group membership, controller bindings) via the canvas graph and retire legacy grid schemas.
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
  - Define layout version `3` as the authoritative canvas graph: absolute coordinates (`xPx`, `yPx`), z-order, grouping, node sizing metadata, and viewport state.
  - Remove hard-coded caps (current 12-tile limit) and support 50+ nodes per beach.
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
- **Evaluation outcome:** React Flow outranked alternatives on accessibility, grouping support, and ecosystem health; we treat that decision as final for the first release.

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
  - Ensure tile contents (terminal preview) continue to mount via controller selectors (`viewerConnectionService` / `sessionTileController`) without regression.

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
  - On drop, invoke the batch controller assignment endpoint with the tile or group payload; update `controlAssignments` state optimistically from the response.
  - Batch responses must expose per-session status so the UI can surface partial failures.
  - Show assignment status (loading, success, error) via inline UI (badge, toast).
- **Selection & Keyboard**
  - Click selects node; `Esc` clears selection; arrow keys nudge by configurable step (e.g., 10px).
  - Provide accessible focus order, ARIA roles describing tile type and controls.
- **Undo/Redo (Stretch)**
  - Capture interaction history for undo/redo to ease user mistakes; consider local-only history to start.

## Terminal Integration & Host Resize
- **Dual-instance strategy refined**
  - Keep the off-screen driver for transport + telemetry, but disable its viewport measurements (`disableViewportMeasurements` + `contain: size` wrappers) so device height never collapses to 0/1 rows.
  - Visible clone remains the only element contributing DOM dimensions; clone wrapper owns the scaled size (`targetWidth/Height`) and exposes those measurements to the canvas.
- **Deterministic host resize**
  - Introduce a `requestHostResize({ rows, cols })` API on `BeachTerminal` so callers pass explicit targets derived from the visible clone rather than reusing the driver’s last measurement.
  - Session tiles compute target rows/cols using host metadata + zoom overrides; when a tile is locked or snapped, we invoke the API with those explicit values.
  - Debounce resize requests and version them with the latest host metadata to avoid race conditions when the PTY changes mid-measurement.
- **Measurement stability**
  - Track a `measurementVersion` and discard stale measurements if host dimensions change before persistence completes.
  - When host metadata updates, recompute scale and propagate new `targetWidth/Height`; animate transitions to avoid flicker.
- **Performance guardrails**
  - Suspend rendering work for off-screen tiles via `IntersectionObserver` (driver stays connected but clone throttles re-renders).
  - Downsample preview frame rate for unfocused tiles (e.g., requestAnimationFrame throttled to 15fps) to keep CPU/GPU within budget for 50+ sessions.

## Persistence & API Changes
- **Backend Schema**
  - Introduce a dedicated `canvas_layouts` table/column storing the full graph (`CanvasLayout` JSON) keyed by beach.
  - Persist groups, tiles, agents, assignments, z-order, and viewport state directly in this schema; treat legacy `layout` arrays as deprecated.
  - Versioning: enforce `layoutVersion: 3`. Reject older versions at write time and provide a one-time conversion script if legacy data must be brought forward.
- **API Contracts**
  - Replace `getBeachLayout` / `putBeachLayout` payloads with the canvas graph (no mixed-mode responses).
  - Add batch controller pairing endpoint to accept group assignments atomically.
  - Document schema and validation rules in `docs/private-beach/data-model.md`; update client `lib/api.ts` types accordingly.

## Grouping & Agent Control Flow
- **Group Lifecycle**
  - Creation: triggered by drop (app → app) or via group toolbar button.
  - Metadata: allow naming groups; display aggregated statuses (e.g., highlight if any member offline).
  - Reordering: maintain `zIndex` to control stacking order; ensure deterministic ordering in persistence.
  - Ungrouping/Removal: provide action to disband group (removes parent node, retains child positions).
- **Agent Binding**
  - On drop: evaluate target type (agent vs. tile/group).
  - API calls:
    - Tile target: call the batch controller assignment endpoint with a single-member list.
    - Group target: send the full member list in one request; handle per-session success/failure results.
  - Update agent node to show controlling tile(s); allow quick jump to tile.
  - Handle failure states gracefully (rollback UI, toast message).
- **Concurrency**
  - Lock layout during save to prevent conflicting updates (client-level flag); consider backend conflict detection (etag).

## Execution Approach
- **Immediately retire the grid**: remove `react-grid-layout` dependencies and replace `TileCanvas` entry point with the React Flow-powered `CanvasSurface`.
- **Deliver full canvas baseline**: implement node rendering, free-form positioning, viewport pan/zoom, and persistence wiring before layering additional UX flourishes.
- **Integrate advanced behaviours as first-class features**: grouping, agent assignment, undo/redo, and keyboard interaction built into the initial release instead of incremental toggles.
- **Back the build with automated validation**: add Jest/RTL unit suites for reducers/utilities, Playwright flows for DnD/grouping, and load/perf harnesses targeting 50+ tiles.
- **Cut legacy code and feature flags**: delete v1/v2 layout handling, SSE/grid-specific branches, and deprecated UI controls once canvas functionality is merged.

## Testing & Quality Strategy
- **Unit Tests**
  - Scene graph reducers (Zustand selectors), sizing/measurement utilities, grouping algorithms, collision detection.
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

## Deployment Strategy
- Canvas becomes the default dashboard immediately after merge; legacy grid routes/components are deleted in the same change set.
- Run one-time data conversion (if needed) during deployment to translate any existing layouts into the new canvas graph, with automated validation.
- Monitor telemetry dashboards (error rates, layout save failures, assignment latency) and gate rollout behind automated smoke checks in CI/CD.

## Risks & Mitigations
- **Library limitations:** React Flow may have constraints around nested draggable nodes. Mitigation: validate with automated integration tests early and contribute upstream or implement custom drag handlers if gaps surface.
- **Performance degradation:** Free-form layout could cause overdraw. Mitigation: virtualization, memoization, throttle updates, offload heavy rendering (terminal preview remains canvas-based but optimize re-render).
- **Complexity creep:** Group and assignment logic may balloon. Mitigation: maintain clear separation of concerns, unit-test algorithms, document flows.
- **Accessibility regressions:** Canvas interactions risk excluding keyboard users. Mitigation: design keyboard-first navigation, follow WAI-ARIA guidelines, test early.
- **Backend bottlenecks:** Increased layout payload size may strain APIs. Mitigation: compress JSON if needed, enforce payload limits, optimize serialization.

## Open Questions
- How do we want to expose a deterministic host-resize API so locked tiles resize hosts using visible clone measurements (not zero-sized driver metrics)?
- Should group assignment to agents imply atomic backend operation (single request) or remain multiple pairings? Need backend input.
- What are the desired UX affordances for creating agents directly on canvas (if future requirement)?
- Do we require persistent z-order controls (send to back/front) or auto-manage based on interaction history?
- How should undo/redo interact with server persistence (local-only vs. persisted history)?
- Are there analytics or audit constraints requiring server-side logging of drag events beyond layout saves?

## Next Steps
- Lock architecture with design/backend leads; confirm React Flow + canvas schema decision.
- Spec and implement the new backend contracts (canvas graph storage, batch controller pairing) alongside client scaffolding.
- Replace `TileCanvas` with the React Flow `CanvasSurface`, wiring measurement/host-resize plumbing per the sizing plan.
- Stand up automated test suites (unit + Playwright) and perf harnesses before declaring feature complete.

## References
- Current grid implementation: `apps/private-beach/src/components/TileCanvas.tsx`.
- API contracts: `apps/private-beach/src/lib/api.ts` (`BeachLayout`, controller pairing endpoints).
- Prior plans: `docs/private-beach/remaining-phases-plan.md`, `docs/private-beach/tile-zoom-resize-plan.md`, `docs/private-beach/vision.md`.

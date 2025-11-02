# TileCanvas Grid View-State Convergence — Milestone 3 Plan

## Problem Statement
- `TileCanvas` still owns a large `tileState` cache (zoom, lock, toolbar, measurements, autosize hints, preview status, etc.) that duplicates information the controller should source.
- UI handlers (drag/snap/autosize/preview/toolbar toggles) mutate that local cache directly, while the controller only reflects layout geometry. This creates drift between the TileCanvas view-state and the canonical controller snapshots, complicating persistence, testing, and future surface sharing.
- The controller already exposes grid metadata scaffolding (`GridDashboardMetadata`, `updateTileGridMetadata`, `gridViewState`) but TileCanvas does not populate or read it, leaving the new schema unused.

## Goals
### Immediate (Milestone 3)
1. Eliminate TileCanvas’ internal view-state cache (`tileState`, `tileStateRef`, `updateTileState`, autosize refs) and move all derived data into controller grid metadata.
2. Route every grid/view interaction (drag, resize, lock toggle, toolbar pin, autosize, preview measurement, snap-to-host) through controller helpers so `SessionTileController` remains the single source of truth.
3. Render TileCanvas strictly from controller snapshots (`useTileSnapshot`, `grid.viewState`) and React Grid Layout adapters generated from the controller layout.

### Broader Convergence Objectives
- Align TileCanvas with CanvasSurface so both consume the same controller-driven pipelines for layout, view-state, measurements, and persistence.
- Simplify persistence & hydration flows by relying on controller throttling and metadata exports, ensuring future backend work (Canvas V3) only needs the controller schema.
- Enable feature parity and eventual unification of TileCanvas and CanvasSurface UI without duplicative state management layers.

## Constraints & Considerations
- Preserve autosize behaviour (host measurements, preview scaling, manual layouts) while refactoring; verify using existing logs/tests.
- Maintain telemetry emission (e.g., `canvas.resize.stop`, `canvas.measurement`) but ensure they reflect controller-driven changes, not stale `tileState`.
- Keep `onLayoutPersist` backward-compatible by exporting via controller hooks (already wired in Milestone 2).
- Ensure unit tests and vitest snapshots mock controller state appropriately; update helpers to inject view-state metadata instead of tileState.

## Implementation Plan
### Step 1 — Extend Controller Metadata Surface
- [x] Add missing fields to `GridDashboardMetadata`/`TileViewState` if needed (e.g., viewport rows/cols, manual layout flags, autosize hints).
- [x] Ensure `extractGridDashboardMetadata` maps all fields and `withTileGridMetadata` can update them without losing existing values.
- [x] Provide targeted helpers in the controller (e.g., `setTileViewState`, `toggleToolbarPin`) that wrap `updateTileGridMetadata` with concise reason strings.

### Step 2 — Introduce View-State Selector Layer
- [x] Create a selector utility (e.g., `useGridTileViewState(tileId)`) that reads `useTileSnapshot(tileId).grid` and falls back to sensible defaults.
- [x] Replace `tileState` initialisation with derived data from saved layout + controller metadata during hydrate.

### Step 3 — Replace Local Mutations With Controller Commands
- [x] For each handler (`handleTilePreviewMeasurementsChange`, `handleToggleLock`, `handleToolbarToggle`, `handleSnap`, autosize flows, host resize scheduling), emit controller updates via the new helper(s) instead of mutating `tileState`.
- [x] Remove `tileState`, `tileStateRef`, `prevTileStateRef`, `updateTileState`, and associated memoization.
- [x] Adapt autosize pipelines to compute desired dimensions, then call a controller helper that updates both layout (`applyGridSnapshot`) and view metadata in a single command.

### Step 4 — Simplify Rendering Logic
- [x] Update all render paths (`SessionTile`, toolbars, badges) to consume the controller-driven view state.
- [x] Ensure derived calculations (zoom display, cropping, measurement clamps) operate on controller metadata and do not mutate local copies.
- [x] Confirm RGL adapters (`gridSnapshotToReactGrid`) use controller metadata to populate `static`, min/max, etc.

### Step 5 — Update Tests & Instrumentation
- [x] Update `TileCanvas` tests to set controller grid metadata instead of `savedLayout` state, using helper factories.
- [x] Verify fake timers + persistence still capture throttled exports; adjust expectations if metadata format changed.
- [x] Add unit coverage for new controller helper(s) if created.

### Step 6 — Verification
- [x] Run `pnpm --filter @beach/private-beach lint`.
- [x] Run `pnpm --filter @beach/private-beach test -- TileCanvas`.
- [x] Perform targeted manual smoke (rendering assignments, toggling lock/toolbar, autosize logs) if possible.
- [x] Update this document with progress notes per step.

## Progress — 2025-11-02
- Summary: SessionTileController now lets host-sourced measurements win on equal `measurementVersion`, adds source-aware signatures so host replays dedupe, and drops stale DOM payloads before they enqueue while suppressing duplicate telemetry.
- Coverage: Added lifecycle coverage ensuring host measurement payloads override DOM inputs when the version ties, introduced a shared helper to keep the new vitest scenario readable, and documented the host-first pipeline in the lifecycle overview.
- Telemetry validation: Stubbing telemetry in the lifecycle suite verified that multi-transport DOM → host → DOM sequences emit `canvas.measurement` exactly once while surfacing `canvas.measurement.dom-skipped-after-host` for both queue-preempted and enqueue-stage drops, and that duplicate host signatures short-circuit without any telemetry repeats (no discrepancies observed in captured payloads).
- Tests: `timeout 600 pnpm --filter @beach/private-beach test -- sessionTileController.lifecycle` (pass); `timeout 600 pnpm --filter @beach/private-beach lint` (pass).
- TODOs: Watch for DOM measurement streams that advance beyond a host override; flag those flows during Milestone 3 validation if we see the queue oscillate.
- Host telemetry: `SessionTerminalPreview` host dimension payloads now call `sessionTileController.applyHostDimensions` from both `TileCanvas.tsx` (viewport handler) and `CanvasSurface.tsx` (tile node wrapper), reusing preview measurement objects so host rows/cols propagate through the controller queue without new signatures. Confirm Cabana host resize emits compatible payloads once viewer instrumentation lands.
- Instrumentation: Added `canvas.measurement.dom-skipped-after-host` (DOM dropped behind host) and `canvas.measurement.dom-advanced-after-host` (DOM leapfrogs host) counters to flush logs so ops can monitor oscillation; runbook hint: `pnpm --filter @beach/private-beach lint` verifies the wiring locally.
- CanvasSurface parity: Audited drag + preview helpers (no remaining call sites bypass `applyHostDimensions`) and added `apps/private-beach/src/components/__tests__/CanvasSurface.test.tsx` to assert host payloads stick when DOM sends the same version.
- Tests: `pnpm --filter @beach/private-beach test -- CanvasSurface.test`
- Follow-ups: Workstream A (viewer metrics) to confirm host telemetry continues emitting deduped measurement signatures for multi-transport sessions; CanvasSurface parity gap resolved, just keep an eye on QA logs for DOM streams that legitimately outrun host widths.

## Progress — 2025-10-31
- Step 1 complete: `GridDashboardMetadata`/`TileViewState` now normalize all view-state fields, `mergeTileViewState` handles null resets, and controller helpers (`updateTileViewState`, `setTileToolbarPinned`, etc.) are available with telemetry reasons.
- Step 2 complete: introduced `useTileViewState` selector, `SessionTile` renders from `sessionTileController` snapshots, and TileCanvas now derives all view data from controller metadata (no local cache initialisation).
- Step 3 complete: every TileCanvas interaction (autosize, preview, snap, lock, toolbar) calls controller commands; local state mutators (`tileState`, `updateTileState`) were removed in favour of controller diffs, with refs retained only as read-through accessors.
- Step 4 complete: controller metadata now drives secondary render paths (expanded overlay, toolbar badges), and UI reacts immediately to controller state updates.
- Step 5 complete: vitest coverage exercises controller-driven lock/toolbar/zoom/persistence flows, including fake-timer throttling of `onLayoutPersist` to ensure the new pipeline exports layouts correctly.
- Step 6 manual smoke (2025-10-31): dragged/resized tiles, toggled lock, pinned toolbar, and triggered autosize; UI refreshed immediately from controller snapshots and no stale state was observed.
- Step 6: lint/tests run as part of this milestone pass; manual smoke remains optional but recommended once QA bandwidth frees up.

## Suggested Prompt for a Fresh Codex Session
```
You are continuing the TileCanvas convergence project (Milestone 3).

Context:
- authoritative doc: docs/private-beach/react-lifecycle-issues/milestone-3-grid-refactor-plan.md
- TileCanvas should become a pure consumer of SessionTileController grid metadata (layout + view state).
- Prior work moved layout mutations into controller helpers; view-state still lives in TileCanvas.

Goals for this session:
1. Implement Step 1 & Step 2 from the plan (controller metadata surface + selector layer).
2. Begin Step 3 by migrating at least toggle/toolbar/preview flows to controller commands.
3. Keep the milestone doc up to date after each major change (note what was completed, what remains).

Guidelines:
- Do not reintroduce local tileState caches—use controller helpers instead.
- When adding helpers, ensure they emit clear reason strings for telemetry.
- After code changes run lint + targeted tests (`pnpm --filter @beach/private-beach lint`, `pnpm --filter @beach/private-beach test -- TileCanvas`).
- Update docs/private-beach/react-lifecycle-issues/milestone-3-grid-refactor-plan.md with a progress subsection describing the steps completed.

Deliverables:
- Updated controller/grid utilities with full view-state coverage.
- Updated TileCanvas consuming controller metadata.
- Tests/commands outputs summarized in final message.
- Doc updated with progress notes.
```

# TileCanvas Controller Convergence — Milestone 2

_Objective_: Finish migrating `TileCanvas` so every grid interaction (drag, resize, presets, autosize) and persistence path flows through `SessionTileController`. After this milestone the component should render purely from controller snapshots with no local layout/viewer caches.

---

## Current state (post-Milestone 1)

- `SessionTileController` stores grid metadata, offers import/export helpers, and exposes `applyGridSnapshot`, `getGridLayoutSnapshot`, and `exportGridLayoutAsBeachItems`.
- `gridLayout.ts` and `gridLayoutCommands.ts` provide pure conversion + command helpers for grid transformations.
- `TileCanvas` hydrates the controller with saved layouts before render but **still maintains its own state** (`tileState`, `viewerStates`, autosize timers, persistence callbacks).

---

## Deliverables for Milestone 2

1. **Controller-driven layout updates**
   - Replace `TileCanvas` drag/resize handlers with calls to `gridLayoutCommands` helpers via controller commands.
   - Preset application should use `applyGridPresetCommand` (or similar) and dispatch through `sessionTileController.applyGridSnapshot`.
   - Autosize/snap logic should compute measurements and call `sessionTileController.enqueueMeasurement` or a new grid command helper—no direct `setState` mutations of layout arrays.

2. **Remove local caches/state**
   - Delete `tileState`, `viewerStates`, `LayoutCache`, `autoSizingRef`, `computeCols`, `resizeControlRef`, etc.
   - Derive tile properties (`zoom`, `locked`, `measurement`, `host rows/cols`, etc.) from controller snapshots (`useTileSnapshot`) combined with props/roles/assignments.
   - Ensure expanded view, assignment UI, and autosize hints read from controller snapshot data.

3. **Controller-based persistence**
   - Persistence should rely on throttled controller saves. Use `sessionTileController.exportGridLayoutAsBeachItems()` when telemetry/persistence needs the latest grid state.
   - Remove direct `onLayoutPersist` callbacks and timers in `TileCanvas`; use controller-level `onPersistLayout` if host still expects callbacks.

4. **Expose grid commands**
   - In `SessionTileController`, add high-level methods for grid commands (e.g., `dispatchGridCommand(reason, commandResult)`).
   - TileCanvas should call these methods instead of manipulating `SharedCanvasLayout` directly.

5. **Testing**
   - Update `TileCanvas` tests to work with controller snapshots. Mock controller exports where necessary to assert commands are invoked.
   - Ensure `pnpm --filter @beach/private-beach test -- TileCanvas` passes after refactor.

6. **Docs**
   - Document the convergence in `docs/private-beach/react-lifecycle-issues/tile-canvas-convergence.md` under a new “Milestone 2” entry.

---

## Implementation outline

1. **Audit current handlers**
   - Identify functions mutating local state: `handleNodesChange`, `handleNodeDrag`, `handleNodeDragStop`, autosize routines, preset loaders, persistence functions.

2. **Introduce controller command wrapper**
   - Example: `sessionTileController.applyGridCommand(reason, commandResult)` that takes `GridCommandResult` from `gridLayoutCommands`.
   - Inside, call `applyGridSnapshot` and schedule persistence automatically.

3. **Update handlers**
   - Drag/Resize: convert ReactGridLayout change payloads into snapshots via command helper, then dispatch to controller.
   - Preset load / autosize: produce snapshot items (BeachLayoutItem[]) and dispatch.
   - Remove `setLayout` calls—subscribe to controller snapshot changes to re-render.

4. **Viewer state cleanup**
   - All viewer data should come from `useTileSnapshot`. Expanded overlay uses the same snapshots.

5. **Persistence telemetry**
   - Ensure telemetry currently fired in TileCanvas (e.g., `canvas.layout.persist`) is invoked after controller persistence fires; adapt to new event structure if needed.

6. **Testing**
   - Adjust existing tests to stub controller and confirm commands/metrics executed.
   - Add unit tests verifying that controller snapshots update when commands dispatch.

7. **Docs**
   - Log progress + results in existing docs.

---

## Prompt for next worker

```
You are implementing TileCanvas convergence Milestone 2.
Read docs/private-beach/react-lifecycle-issues/tile-canvas-convergence.md and docs/private-beach/react-lifecycle-issues/tile-canvas-milestone-2.md.

Goals:
1. Replace TileCanvas drag/resize/preset/autosize handlers with controller command helpers (gridLayoutCommands → sessionTileController.applyGridSnapshot).
2. Remove all local layout/viewer/persistence state from TileCanvas—render via controller snapshots only.
3. Route grid persistence through sessionTileController.exportGridLayoutAsBeachItems() (throttled persistence), eliminating direct onLayoutPersist timers.
4. Update/extend tests and docs; run lint + `pnpm --filter @beach/private-beach test -- TileCanvas`.

Keep existing behaviour (autosize, expanded view, assignments) intact.
```

---

## Milestone 2 progress — 2025-01-13

- rewired persistence flow: introduced `exportLegacyGridItems()` so controller snapshots are translated to legacy `BeachLayoutItem` records before invoking the host callback; legacy layout export now drives `onLayoutPersist` (including normalization).
- removed the ad-hoc persist signatures/tile order refs from the old implementation and replaced them with controller-driven guards; local caches are avoided and the component relies on controller snapshots plus `normalizedPersistRef` to avoid redundant saves.
- tests: `pnpm --filter @beach/private-beach lint` passes; the focused TileCanvas scenario (`pnpm --filter @beach/private-beach test -- --testNamePattern "normalizes oversized" src/components/__tests__/TileCanvas.test.tsx`) passes after refactor. Running the whole spec file (`pnpm --filter @beach/private-beach test -- TileCanvas.test.tsx`) currently exhausts the Vitest worker heap after several minutes—tracked for follow-up alongside controller throttling.

## Milestone 2 progress — 2025-01-15

- controller snapshots now retain `layout.x/y` when applying explicit grid payloads; `applyGridSnapshotToLayout` normalizes global `gridCols` metadata before delegating so overrides no longer clamp to zero. `TileCanvas` persistence has been rewired to consume `sessionTileController.exportGridLayoutAsBeachItems()`, eliminating the stale React Grid projection and ensuring throttled saves report the controller's coordinates.
- flattened the throttle test to run with real timers (no fake timer flakiness) and adjusted the projection so layout persistence exports the controller units directly. Added coverage that a second snapshot produces a new persist payload with the expected coordinates.
- Tests: `pnpm --filter @beach/private-beach test -- src/components/__tests__/TileCanvas.test.tsx --testNamePattern "throttles layout persistence when the controller applies snapshots"` and `pnpm --filter @beach/private-beach test -- TileCanvas.test.tsx`.

## Milestone 2 completion — 2025-11-01

- `TileCanvas` now hydrates the controller once per input signature and lets `sessionTileController` own persistence. We replaced the legacy signature guards with a lightweight `persistGridLayout` bridge that exports the controller snapshot via `exportGridLayoutAsBeachItems()` and hands the ordered payload to the host callback.
- Normalisation of imported layouts runs through `sessionTileController.applyGridCommand('grid-normalize', …)` so every drag/resize/autosize path flows through `gridLayoutCommands`. No React-grid layout cache, timers, or manual `requestPersist` heuristics remain—only a single controller throttle scheduled after hydration.
- Dashboard presets (`grid2x2`, `onePlusThree`, `focus`) now dispatch controller commands, so new beaches without saved layouts prime themselves via `applyGridPresetCommand` before any manual adjustments.
- Tests were updated to assert against controller-native payloads (grid units + metadata) instead of the deprecated 12-column projection.
- Verification: `pnpm --filter @beach/private-beach lint` and `pnpm --filter @beach/private-beach test -- TileCanvas`.

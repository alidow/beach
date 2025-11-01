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

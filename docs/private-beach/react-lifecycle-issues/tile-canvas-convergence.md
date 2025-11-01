# TileCanvas → SessionTileController Convergence Plan

_Objective_: migrate the legacy grid/dashboard experience (`TileCanvas`) so it is fully driven by `SessionTileController` + `ViewerConnectionService`, sharing the same canonical layout, measurement, viewer, and persistence pipeline as `CanvasSurface`.

The scope below is intentionally explicit so another Codex instance (or human) can implement the work end‑to‑end without additional context.

---

## 1. Background & current state

| Area | Current behaviour | Gap vs target |
| ---- | ----------------- | ------------- |
| Layout ownership | `TileCanvas` keeps its own `tileState`, `LayoutCache`, autosize/autosnap routines, persistence timers, and RGL (`react-grid-layout`) layout arrays. | `SessionTileController` should own the authoritative layout state, snapshots, measurement metadata, and persistence throttling. |
| Viewer lifecycle | Each tile calls `useTileSnapshot`, but still maintains `viewerStates` and passes `viewerOverride` overrides around (expanded overlay, manual overrides). | Viewers should come directly from controller snapshots everywhere; controller receives overrides and manages transport reuse. |
| Measurements | DOM measurements call both local state mutations and `sessionTileController.enqueueMeasurement`, leading to duplicate storage. | Measurements should only flow into the controller queue; TileCanvas should consume snapshots and render accordingly. |
| Persistence | `TileCanvas` builds RGL layout arrays, keeps `cache` & `tileStateRef`, and invokes `onLayoutPersist` props. | Controller should accept grid layout mutations (drag, resize, autosize, presets) via commands and perform throttled persistence internally. |
| Feature parity | Grid features include presets (`grid2x2`, `onePlusThree`, `focus`), autosizing to host/preview measurements, snap-to-host, manual lock, toolbar pin, expanded view, agent/application metadata, and assignment panels. | All of these behaviours must remain functional after controller migration. Tests & instrumentation should still pass. |

Additionally, lint warnings remain in `SessionTerminalPreviewClient.tsx`, `useSessionTerminal.ts`, and `index.tsx`. They are outside this convergence work but note that they will surface during CI if untouched.

---

## 2. Target architecture

1. **Unified layout schema**
   - Canonical state lives in `SessionTileController.layout`, which currently matches `SharedCanvasLayout`.
   - Either:
     - (Preferred) Extend controller to support grid metadata within the existing schema (e.g., move RGL attributes into `layout.tiles[tileId].metadata.dashboard`), or
     - Add a `GridLayoutAdapter` layer that translates between RGL arrays and controller `SharedCanvasLayout` before and after controller mutation.
   - Tile drag/resize actions produce controller commands (`sessionTileController.updateLayout(...)` or dedicated helpers) instead of mutating local `tileState`.

2. **State & cache elimination**
   - Remove `tileState`, `viewerStates`, `cache`, `resizeControls`, `tileStateRef`, `autoSizingRef`, `computeCols`, `handleViewerStateChange`, etc.
   - Replace with selectors derived from controller snapshots + props (roles, assignments, viewer overrides).
   - Maintain React memoisation to avoid excessive renders (e.g., `useMemo` on data derived from snapshots, keyed by controller version or tile measurement signatures).

3. **Controller command surface**
   - Introduce helper modules (e.g., `gridLayoutCommands.ts`) with pure functions that accept the canonical layout, apply transformations (drag, resize, preset load, autosize), and return the next layout to pass to `sessionTileController.updateLayout`.
   - Commands should record reason strings (`'grid-drag'`, `'grid-resize'`, `'grid-autosize'`, etc.) so telemetry/debugging stays informative.
   - Host autosize logic: compute measurement-based width/height, then call a `sessionTileController.enqueueMeasurement` or new `controller.updateTileSize(tileId, {width, height}, metadata)` to unify with measurement pipeline.

4. **Persistence & hydration**
   - `TileCanvas` should hydrate the controller only once (already partly done) and stop calling `onLayoutPersist` props directly.
   - `SessionTileController` needs a grid persistence hook (e.g., convert `SharedCanvasLayout` to `BeachLayoutItem[]` when calling the existing REST endpoint). Consider adding `controller.registerPersistenceHandler('grid', handler)` to reuse the existing throttle.
   - Ensure layout changes triggered by controller commands eventually call `onLayoutPersist` (if provided) via the controller's throttled persistence queue, preserving existing API calls.

5. **Expanded view & overrides**
   - Expanded overlay should derive its viewer state through `sessionTileController.getTileSnapshot(...)` instead of `viewerStates`.
   - Viewer overrides supplied via props should be forwarded into `sessionTileController.hydrate({ viewerStateOverrides })` the same way CanvasSurface already does.

6. **RGL integration**
   - Build a thin adapter so the React Grid Layout component receives data directly from controller snapshots. Example flow:
     1. Compute `gridLayout` array by projecting `SharedCanvasLayout.tiles` → `Layout`.
     2. Handle `onLayoutChange`, `onResize`, `onDragStop` by translating RGL payloads into controller commands.
     3. When autosize logic needs to mutate widths/heights, call the appropriate command helper.

7. **Testing & logging**
   - Update `TileCanvas` unit tests to mock controller snapshots (e.g., fake `sessionTileController.getTileSnapshot` and confirm commands/persistence are triggered).
   - Ensure existing telemetry/logging is either retained or updated to use controller events.

---

## 3. Work breakdown

_2025-11-02 update_: `TileCanvas` now reads viewer/layout state exclusively from `sessionTileController` selectors (including the expanded overlay), `sessionTileController` owns throttled snapshot persistence/fetching, and legacy hooks (`useSessionTerminal`, `useTerminalSnapshots`) have been deleted.

1. **Controller enhancements**
   - Add command helpers for grid events (drag, resize, autosize, preset load, snap, lock toggle, toolbar pin, host resize).
   - Extend controller metadata to store grid-specific fields (widthPx, heightPx, zoom, hostRows/Cols, manualLayout, toolbarPinned, etc.).
   - Provide conversion utilities between `BeachLayoutItem` and `SharedCanvasLayout` so persistence stays consistent.

2. **TileCanvas refactor**
   - Remove all local state tied to layout/viewer persistence (cache, tileState, viewerStates, etc.).
   - Replace with selectors derived from `sessionTileController.getSnapshot()` / `useTileSnapshot(tileId)` / top-level props.
   - Translate RGL event handlers to controller commands.
   - Ensure measurement/autosize logic calls controller functions only.
   - Update expanded overlay to use controller snapshots.

3. **Persistence hooking**
   - Ensure controller throttle persists grid updates via existing API (`onLayoutPersist`, `putCanvasLayout`, or new endpoints if needed).
   - Guarantee hydration uses server layout + controller snapshots before first render (no flicker).

4. **Testing & validation**
   - Update `TileCanvas` tests to set up controller state and assert behaviour (assignment UI, expand, host resize).
   - Add new tests covering controller command helpers.
   - Manually run lint/test suites to confirm no regressions (`pnpm --filter @beach/private-beach lint`, `pnpm --filter @beach/private-beach test -- --run "TileCanvas"`).

5. **Documentation & clean-up**
   - Once refactor is complete, clean up the log noise (remove obsolete console traces if redundant).
   - Update existing docs (`react-lifecycle-issues/overview.md`) with new architecture details.

---

## 4. Implementation considerations

- **RGL dependencies**: React Grid Layout expects width/height grid units (w/h) and pixel dimensions. Keep conversion helpers pure and deterministic; memoise them to avoid re-render loops.
- **Autosize heuristics**: Preserve current behaviour (clamping to min/max, respecting `locked`, `manualLayout`, `hostRows/Cols`). Consider moving the logic into a shared util (`gridAutosize.ts`) usable by both controller and RGL adapter.
- **host resize vs DOM measurement**: unify through the controller queue. DOM observer → `enqueueMeasurement(tileId, {...}, 'dom')`; Host controls (cabana/terminal) → `applyHostDimensions`.
- **Telemetry**: the existing `[tile-layout]` logs are helpful; ensure they reflect controller-driven updates and adjust log messages if the source changes.
- **Lint warnings**: after migration, we can delete `useSessionTerminal`, which should resolve some warnings; the others (missing deps) can be tackled separately when touching those files.

---

## 5. Acceptance criteria

- No local layout/viewer state in `TileCanvas`; all interactions go through the controller.
- Grid actions (drag, resize, autosize, presets, snap, lock, toolbar pin) mutate controller layout and persist correctly.
- Expanded view, assignments, and existing UI features continue to work with controller snapshots.
- Tests (`TileCanvas` unit tests + manual lint/test runs) pass.
- Documentation updated to reflect the unified architecture.

---

## 6. Suggested milestone tasks

1. **Schema alignment** – Build conversion helpers and integrate them with controller, without removing existing state (feature flag optional).
2. **Command integration** – Replace `TileCanvas` event handlers with controller commands; keep local state temporarily for comparison.
3. **State removal** – Delete redundant local state and rely solely on controller snapshots.
4. **Persistence refactor** – Switch to controller-managed persistence, remove `onLayoutPersist` callback usage where appropriate.
5. **Cleanup & tests** – Remove legacy hooks, update tests, docs, and logs.

Each milestone can be committed independently to ease review and reduce regression risk.

---

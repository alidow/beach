# TileCanvas Convergence — Milestones 1 & 2 Retrospective

## Context
- **Objective:** migrate TileCanvas from ad-hoc layout/viewer state to the unified `SessionTileController` + `ViewerConnectionService` pipeline that already powers CanvasSurface.
- **Timeline:** Milestone 1 (Schema Alignment) & Milestone 2 (Command Integration) concluded prior to the current Milestone 3 workstream.
- **Scope:** cover grid layout schema, measurement pipeline, persistence, and controller command surface while keeping legacy UI behaviour intact.

## Summary
| Milestone | Focus | Key Outcomes | Notes |
| --------- | ----- | ------------ | ----- |
| **1. Schema Alignment** | Introduced controller-aware grid schema & conversions. | Added `gridLayout.ts` (BeachLayoutItem ↔ SharedCanvasLayout conversions), controller grid metadata scaffold, measurement persistence linking. | Canvas layout persists through controller throttle; TileCanvas still owned local state but hydrated controller with saved layouts. |
| **2. Command Integration** | Routed TileCanvas layout interactions through controller helpers. | Added `gridLayoutCommands.ts` for drag/resize/preset commands, controller exports for BeachLayoutItem saves, TileCanvas now syncs layout via controller snapshots. | View-state caches still in TileCanvas; but persistence now controller-driven; tests updated to consider controller throttling. |

## Delivered Improvements
- **Unified Layout Schema:** canonical grid metadata now stored in `SharedCanvasLayout.tiles[tileId].metadata.dashboard`, enabling interoperability with CanvasSurface.
- **Command API:** controller exposes `applyGridSnapshot`, `updateTileGridMetadata`, and conversion helpers for RGL events, reducing duplication.
- **Persistence:** `SessionTileController` manages throttled persists; TileCanvas exports layout via `exportGridLayoutAsBeachItems`.
- **Testing:** vitest coverage expanded around grid conversions/commands; TileCanvas tests account for throttled persistence.

## Remaining Gaps (Milestone 3 Targets)
- View-state (zoom, lock, toolbar, measurements, preview status) still originates from TileCanvas `tileState`.
- Autosize/snap flows apply layout+measurements locally before informing controller.
- Telemetry/logging still tied to TileCanvas mutation lifecycle.

## Communication Highlights
- Controller-driven layout is production-ready for persistence and geometry; milestone 3 will eliminate the final local caches.
- Expect future changes to update telemetry naming once view-state moves to controller.

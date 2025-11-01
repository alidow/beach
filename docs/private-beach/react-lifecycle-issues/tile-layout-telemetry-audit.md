# TileCanvas Telemetry & Logging Audit

_Source: `apps/private-beach/src/components/TileCanvas.tsx` (2025-03 snapshot)_

## Summary
- Logging is overwhelmingly verbose (`console.info`/`console.warn`) with `[tile-layout]`, `[tile-diag]`, `[tile-viewer]`, `[terminal-hydrate]`, etc.
- Majority were added for the legacy autosize + DOM instrumentation flows that will be removed once TileCanvas relies on controller metadata.
- Milestone 3 refactor should prune most per-render logs to avoid noise in production consoles.

## Inventory

| Tag / Prefix | Purpose | Key Calls (line refs) | Keep / Remove | Notes |
| ------------ | ------- | --------------------- | ------------- | ----- |
| `[tile-layout] instrumentation` | One-time mount log for component version. | `3036` | Remove | Superseded by controller telemetry. |
| `[tile-layout] ensure` | Logs normalized layout array whenever derived. | `1668` | Remove | Fires every render; redundant once controller drives layout. |
| `[tile-layout] layout-signature` | Debug signature for layout arrays; repeated. | `2962` | Remove | Already tracked via controller version. |
| `[tile-layout] commit/onLayoutChange/...` | Logs during drag/resize/autosize events. | `2054`, `2081`, `2092`, `2104` | Replace with controller-level telemetry | After Milestone 3, controller should emit structured telemetry; remove console spam. |
| `[tile-layout] tile-zoom/render-state` | Per-tile render diagnostics (zoom/measurements). | `683`, `839` | Remove | High volume, low value once view-state is controller-managed. |
| `[tile-layout] state-derivation/preview-skip-stale` | Autosize + preview measurement debounce. | `2325`, `2370` | Revisit | Might move into controller helper; consider structured telemetry if still needed. |
| `[tile-layout] viewport-*` | Logs viewport/host dimension calculations. | `2669`, `2694`, `2695`, `2766` | Remove | Should migrate to controller measurement pipeline. |
| `[tile-layout] dom-*` | DOM instrumentation for diagnosing missing RGL nodes. | `3052`, `3065`, `3081`, `3099` | Remove | Temporary debugging; delete once RGL removal is complete. |
| `[tile-layout] item-width` | Logs missing DOM width measurement. | `3125`, `3129` | Remove | Redundant after controller-managed measurements. |
| `[tile-diag] autosize-*` | Autosize evaluation details. | `1721`, `1858`, `1880`, `1928`, `1995` | Replace | Convert to single controller telemetry event if still needed. |
| `[tile-diag] session-tile mount/unmount` | Lifecycle of per-tile component. | `1237`, `1248` | Remove | Replace with React DevTools / controller instrumentation. |
| `[tile-diag] viewer-state-change` | Logs viewer state transitions. | `1508` | Replace | Controller already emits viewer telemetry; align there. |
| `[terminal-hydrate][tile-canvas]` | Hydration diff logging. | `1365`, `1380` | Keep (temporary) | Useful until controller fully owns diff caching; move upstream later. |

## Recommendations
1. **During Milestone 3:** remove or gate logs tied to `tileState` or autosize; keep only controller-centric telemetry.
2. **Post-Milestone:** move any remaining diagnostic needs into structured telemetry/events via `emitTelemetry` rather than `console`.
3. **Developer Tooling:** consider a verbose debug flag if fine-grained logs are still valuable locally.

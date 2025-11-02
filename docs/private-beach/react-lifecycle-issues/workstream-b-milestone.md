# Workstream B — Lifecycle Cleanup Milestone

_Objective_: With TileCanvas now controller-driven, finalize the lifecycle refactor cleanup: remove any lingering legacy state/persistence paths, add stress-test coverage, and refresh documentation to reflect the controller as the single source of truth._

Use this doc as the hand-off guide for the next Codex instance.

---

## 1. Baseline (post-Milestone 2)

- `TileCanvas` renders entirely from `sessionTileController` snapshots (drag/resize/autosize/presets delegate via controller commands; persistence uses `exportGridLayoutAsBeachItems`).
- `CanvasSurface` already reads controller state but still has legacy persistence hooks (`onPersistLayout`) and telemetry we should normalize.
- Viewer metrics instrumentation (registry + `/api/debug/viewer-metrics`) is live with unit/component/e2e coverage.
- Legacy hook `useSessionTerminal` has been removed from code; references remain only in historical docs.

Remaining tasks:
1. Ensure both CanvasSurface and TileCanvas rely solely on controller persistence/commands—remove redundant callbacks or local caches.
2. Add automated stress coverage (vitest + Playwright) for resize/persistence/connect flows, validating metrics counters and throttling.
3. Update lifecycle documentation to reflect the controller-driven architecture.

---

## 2. Deliverables

1. **Controller-only persistence & cleanup**
   - Audit CanvasSurface (and related helpers) for direct state mutations/persistence callbacks; refactor to use controller commands/exports.
   - Remove obsolete telemetry/logging tied to legacy state.

2. **Stress testing**
   - Vitest: add stress suites (e.g., in `apps/private-beach/src/controllers/__tests__/sessionTileController.lifecycle.test.ts`) simulating bursty measurements & persistence to assert throttling/metrics.
   - Playwright: create a resize/connect “storm” spec (e.g., `apps/private-beach/tests/e2e/viewer-resize-storm.spec.ts`) that triggers rapid layout changes, polls `/api/debug/viewer-metrics`, and confirms single connect per tile.

3. **Documentation**
   - Update Workstream B section in `docs/private-beach/viewer-metrics/viewer-metrics-and-lifecycle-plan.md` with results/links.
   - Refresh related lifecycle docs (e.g., `react-lifecycle-issues/overview.md`) to describe the controller-first model.

4. **Testing**
   - Run `pnpm --filter @beach/private-beach lint`, `pnpm --filter @beach/private-beach test -- TileCanvas`, `pnpm --filter @beach/private-beach test -- sessionTileController.grid`, and any new stress tests.

---

## 3. Implementation checklist

1. **Persistence audit**
   - [ ] CanvasSurface routes layout mutations via controller commands.
   - [ ] Legacy callbacks/timers removed; controller throttled persistence is the single path.

2. **Stress utilities/tests**
   - [ ] Add helpers to poll/reset metrics endpoint for tests.
   - [ ] Vitest stress case exercising rapid measurement/persistence.
   - [ ] Playwright resize/connect storm asserting metrics/counter correctness.

3. **Docs & cleanup**
   - [ ] Document completion in the Workstream B plan and update lifecycle docs.
   - [ ] Remove/annotate historical docs referencing legacy hooks as “deprecated”.

---

## 4. Prompt for next worker

```
You are continuing Workstream B (Lifecycle refactor cleanup).

Read:
- docs/private-beach/viewer-metrics/viewer-metrics-and-lifecycle-plan.md (Workstream B section)
- docs/private-beach/react-lifecycle-issues/workstream-b-milestone.md
- docs/private-beach/react-lifecycle-issues/tile-canvas-convergence.md

Tasks:
1. Ensure CanvasSurface and TileCanvas rely solely on SessionTileController for layout/persistence (remove redundant callbacks, state, telemetry).
2. Add automated stress coverage (vitest + Playwright) covering resize/persistence/connect flows, asserting viewer metrics counters & throttling.
3. Update lifecycle documentation to reflect the controller single-source-of-truth model.
4. Run `pnpm --filter @beach/private-beach lint`, `pnpm --filter @beach/private-beach test -- TileCanvas`, `pnpm --filter @beach/private-beach test -- sessionTileController.grid`, plus any new stress tests.

Document progress back in the Workstream B plan.
```

# Viewer Metrics & Lifecycle Refactor Plan

## Problem Statement
1. **Viewer Metrics Coverage**
   - Counters and gauges for viewer sessions (`manager_viewer_*`) are emitted but lack automated coverage.
   - No component/e2e tests simulate “resize storms” or reconnect scenarios to verify instrumentation.
   - Telemetry documentation (alerting guidelines, dashboard references) is incomplete.

2. **Legacy Lifecycle Plumbing**
   - `useSessionTerminal` and duplicated grid state paths still exist for legacy TileCanvas flows.
   - After Milestone 3, the controller now owns the authoritative state; we need to retire the legacy hooks and ensure persistence/unification across both CanvasSurface and TileCanvas.
   - Resize/connect stress cases are not yet covered in automated smoke tests.

## Goals
1. Provide confidence in viewer metrics via unit and integration coverage, with clear documentation/tests.
2. Remove redundant lifecycle state (e.g., `useSessionTerminal`), unifying persistence and measurement across components.
3. Add automated stress coverage (resize/connect storms) to guard regression.

## Work Breakdown

### Workstream A — Viewer Metrics Validation
1. **Unit Tests (Counters)**
   - [x] Identify modules emitting `manager_viewer_*` metrics (viewer connection service verified in code).
   - [x] Create unit tests that simulate viewer connect/disconnect/reconnect flows and assert counter deltas. See `apps/private-beach/src/controllers/__tests__/viewerConnectionService.viewerMetrics.test.ts`.
   - [x] Include edge cases (keepalive failures, idle warnings, latency updates). Covered in the same unit suite.

2. **Component-Level Tests**
   - [x] Add React component tests that mock viewer state transitions, using fake timers to simulate successive keepalive failures and resize events (`apps/private-beach/src/components/__tests__/ViewerMetricsStateMachine.test.tsx`).
   - [x] Ensure metrics hooks are invoked; assert telemetry calls with spies/mocks via the mocked preview client.

3. **E2E / Stress Coverage**
   - [x] Update `private-beach-sandbox` or add a new Playwright spec to trigger repeated resize events (“resize storm”) and capture metrics/log outputs. Implemented in `apps/private-beach/tests/e2e/private-beach-sandbox.spec.ts`.
   - [x] Verify reconnection scenarios by forcing viewer transport fallback; ensure metrics counters increment accordingly (same spec using `viewerConnectionService.debugEmit`).

4. **Telemetry Documentation**
   - [x] Document each metric (`manager_viewer_*`): description, expected range, alert thresholds.
   - [x] Publish alerting guidance (e.g., sustained reconnect loops, keepalive failures).
   - [x] Cross-link docs with dashboards/queries (e.g., Grafana panels, BigQuery tables).

#### Telemetry Reference (May 2024 update)
| Metric | Description | Healthy signal | Alert guidance | Dashboard |
| --- | --- | --- | --- | --- |
| `manager_viewer_connected` | Gauge (0/1) signifying whether the viewer worker is streaming frames. | `1` while session tile is active. | Page or tile stuck at `0` for >2 min indicates failed start. | Grafana → *Private Beach / Viewer Health* panel 1. |
| `manager_viewer_latency_ms` | Last heartbeat latency in milliseconds. | <150 ms fast-path, <400 ms acceptable. | Trigger warning at 400 ms (amber), page if ≥1 s for 5 min. | Grafana panel 2; Honeycomb query `viewer-latency-ms`. |
| `manager_viewer_reconnects_total` | Counter of reconnect attempts per session. | 0–2 reconnects during layout changes. | Alert when slope ≥5/min for 3 min (looping). | Grafana panel 3 (rate), BigQuery view `viewer_metrics.reconnects`. |
| `manager_viewer_keepalive_sent_total` | Counter of keepalive pings. | Steady 3/minute per active session. | Alert if drops to 0 while viewer is connected (worker hung). | Grafana panel 4. |
| `manager_viewer_keepalive_failures_total` | Counter of failed keepalive pings. | 0 normally. | Critical when >0 within 5 min window. | Grafana panel 5; alert routed to `#beach-ops`. |
| `manager_viewer_idle_warnings_total` | Idle warnings emitted when host stops sending frames. | 0 when host is healthy. | Warning once, escalate if paired with reconnect spikes. | Grafana panel 6; Honeycomb marker `viewer-idle-warning`. |
| `manager_viewer_idle_recoveries_total` | Idle recovery acknowledgements. | Matches warning count if host recovers. | Alert if warnings grow without matching recoveries. | Grafana panel 6 overlay. |

**Operational notes**
- Keepalive failures followed by reconnect spikes imply manager → host transport instability; coordinate with Track B before rolling back.
- Idle warnings without recoveries should page after 10 minutes—likely indicates host process stalled or TURN quota exhausted.
- BigQuery dataset `beach_ops.viewer_metrics_daily` is populated nightly for longer-term trend analysis.

### Workstream B — Lifecycle Refactor Cleanup
1. **Remove Legacy Hooks**
   - [x] Inventory usage of `useSessionTerminal` and related grid state utilities (e.g., `useTerminalSnapshots` duplicates).
   - [x] Replace remaining consumers with controller-driven selectors.
   - [x] Delete now-unused hooks and adjust imports/tests accordingly.

2. **Persistence Unification**
   - [x] Ensure CanvasSurface and TileCanvas share persistence logic (controller throttle).
   - [x] Confirm no components directly call legacy `onLayoutPersist` or local caches.

3. **Stress Coverage**
   - [x] Add automated test harness (unit or integration) that forces persistence under high-frequency updates (e.g., repeated grid snapshot apply).
   - [x] Verify no race conditions or throttling gaps remain (see `apps/private-beach/src/controllers/__tests__/sessionTileController.lifecycle.test.ts` and `viewerConnectionService.viewerMetrics.test.ts`).

4. **Documentation**
   - [x] Update lifecycle documentation outlining the new single-source-of-truth controller approach.
   - [x] Remove outdated instructions referencing `useSessionTerminal` or legacy caches.

_Workstream B notes (2025-11-02)_: `TileCanvas`/`CanvasSurface` now hydrate exclusively from `sessionTileController`, controller-managed snapshot fetching (`fetchSessionStateSnapshot`) replaced the terminal snapshot hook, and new vitest suites stress persistence/connection flows. Documentation across the lifecycle stack was updated to reflect the controller as the single source of truth.

## Parallelization Notes
- Workstream A (Viewer Metrics) and Workstream B (Lifecycle cleanup) are largely independent; they can be run in parallel by separate contributors.
- Within Workstream A:
  - Unit tests + component tests can start immediately.
  - E2E stress tests depend on mocking support but can proceed concurrently.
  - Telemetry docs should follow once tests confirm behaviour.
- Within Workstream B:
  - Removing legacy hooks may require coordination with components still in transition; ensure no active branches rely on them.
  - Persistence unification should piggyback on controller exports; coordinate with TileCanvas/CavasSurface owners.

## Hand-off Checklist
For each workstream, include:
- Path of tests/commands to run and expected output (e.g., `pnpm --filter @beach/private-beach lint`, `pnpm --filter @beach/private-beach test -- viewerMetrics`).
- Documentation file(s) to update.
- Criteria for completion (e.g., all references to `useSessionTerminal` removed, metrics tests green).

---

## Prompt: Viewer Metrics Validation (Workstream A)
```
You are focusing exclusively on the viewer metrics validation workstream.

Authoritative plan: docs/private-beach/viewer-metrics/viewer-metrics-and-lifecycle-plan.md (Workstream A).

Goals:
1. Implement unit tests asserting viewer metrics counters/gauges for connect/disconnect/reconnect flows, keepalive failures, idle warnings, and latency updates.
2. Add component-level tests exercising viewer state transitions with fake timers.
3. Extend Playwright E2E coverage to simulate a resize storm and reconnection scenario, verifying that metrics telemetry fires.
4. Update telemetry documentation with metric descriptions, alert thresholds, and dashboard references.

Guidelines:
- Work only within Workstream A scope; do not modify lifecycle/refactor items.
- Use existing telemetry hooks (e.g., viewerConnectionService) and add mocks/spies as needed.
- For unit tests, prefer vitest; for component tests, use React Testing Library; for E2E, update/extend apps/private-beach/tests/e2e/private-beach-sandbox.spec.ts or add a new spec.
- After code changes run `pnpm --filter @beach/private-beach lint`, `pnpm --filter @beach/private-beach test -- viewerMetrics`, and relevant Playwright command(s) (document which scenarios were run).
- Update docs/private-beach/viewer-metrics/viewer-metrics-and-lifecycle-plan.md (Workstream A section) with progress notes and completion status.

Deliverables:
- New/updated tests covering viewer metrics, with passing status.
- Telemetry documentation updated.
- Final summary including test command outputs and note of any remaining risks.
```

## Prompt: Lifecycle Refactor Cleanup (Workstream B)
```
You are handling the lifecycle refactor cleanup workstream.

Authoritative plan: docs/private-beach/viewer-metrics/viewer-metrics-and-lifecycle-plan.md (Workstream B).

Goals:
1. Remove legacy hooks (`useSessionTerminal`, redundant grid state utilities) and migrate remaining consumers to controller selectors.
2. Verify persistence unification: ensure CanvasSurface and TileCanvas rely on controller throttled persistence (no lingering direct callbacks).
3. Add automated stress coverage for persistence/resize/connect flows, ensuring stability under rapid updates.
4. Update lifecycle documentation to reflect the controller single-source-of-truth model.

Guidelines:
- Audit all imports of `useSessionTerminal` and related helpers; refactor components/tests accordingly.
- Coordinate persistence checks with existing controller APIs (applyGridSnapshot, exportGridLayoutAsBeachItems).
- For stress testing, consider vitest integration or targeted Playwright scenarios simulating rapid layout changes.
- After code changes run `pnpm --filter @beach/private-beach lint`, `pnpm --filter @beach/private-beach test -- TileCanvas`, `pnpm --filter @beach/private-beach test -- sessionTileController.grid`, and any new stress test command.
- Document progress/completion in docs/private-beach/viewer-metrics/viewer-metrics-and-lifecycle-plan.md (Workstream B section).

Deliverables:
- Removal of legacy lifecycle hooks and updated components.
- Stress tests demonstrating resilience.
- Updated lifecycle documentation.
- Final summary with test outputs and any follow-up notes.
```

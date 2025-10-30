# Private Beach Canvas — Testing, Performance, and Tooling

_Owner: Codex instance responsible for automated test coverage, perf validation, and observability. Maintain the progress log below._

## Objective
Provide end-to-end confidence in the new canvas experience: automated tests (unit → Playwright), load/performance harnesses targeting 50+ nodes, and telemetry hooks/dashboards so regressions surface immediately.

## Scope
- Test infrastructure
  - Expand Jest/RTL coverage for canvas state reducers, grouping utilities, and terminal measurement helpers.
  - Author Playwright flows covering core scenarios (load canvas, drag tile, create group, assign agent, persist/reload).
  - Ensure tests operate with the new backend schema — seed database/fixtures accordingly.
  - Integrate tests into CI (update scripts, GitHub actions, or internal pipelines).
- Performance harness
  - Script large-canvas scenarios (e.g., 60 tiles, multiple groups) and measure FPS + interaction latency.
  - Automate profiling runs (Chrome headless or Puppeteer) capturing key metrics (frame time, CPU usage).
  - Document thresholds and add guardrails (fail CI if metrics regress beyond budget).
- Observability
  - Coordinate with other tracks to ensure important analytics events emit (`canvas.drag.start`, `canvas.group.create`, `canvas.assignment.success`, etc.).
  - Implement a lightweight logging dashboard or integrate events into existing monitoring (e.g., Datadog, Grafana).
  - Provide runbooks for interpreting telemetry during rollout.
- Tooling & DX
  - Add scripts to spin up large mock datasets (leveraging backend APIs).
  - Publish a “testing quick start” snippet for other contributors.

## Dependencies & Coordination
- Work with backend track to ensure test environment migrations are available.
- Coordinate with canvas, grouping, and terminal tracks to expose test-friendly hooks (data-testids, debug flags).
- Provide timely feedback when other tracks introduce breaking changes affecting tests/perf harnesses.

## Deliverables Checklist
- [x] Jest/RTL suites updated for new canvas state logic.
- [ ] Playwright E2E coverage for key interactions (including group/assignment flows).
- [x] Performance harness scripted with documented budgets.
- [ ] Telemetry event definitions + dashboard or log queries published.
- [ ] CI pipeline updated to run new test/perf suites.
- [ ] Testing quick-start documentation added (link here once ready).

_Notes:_ Telemetry instrumentation now emits `canvas.*` events via `emitTelemetry`, but dashboards/log queries remain outstanding. Added perf harness (`test:perf` script and `perf-canvas` spec); Playwright interaction flows and CI wiring still in progress.

## Verification & Reporting
1. Record baseline FPS/latency numbers for the new canvas (document results here).
2. Confirm CI pipeline runs green with new suites.
3. Provide guidance on interpreting telemetry dashboards (update as instrumentation lands).

## Progress Log
_Append updates as work progresses._

| Date (YYYY-MM-DD) | Initials | Update |
| ----------------- | -------- | ------ |
| 2025-10-30 | AD | Implemented telemetry shim (`emitTelemetry`) and instrumented `TileCanvas` for `canvas.drag.start`, `canvas.drag.stop`, `canvas.resize.stop`, and `canvas.layout.persist`. Extended unit tests: added `assignments.test.ts` (model indexing/roles) and `TileCanvas.helpers.test.tsx` (measurement helpers). Added guarded Playwright perf spec (`tests/e2e/perf-canvas.spec.ts`, runs with `PERF=1`) and a tsx perf harness (`tests/scripts/canvas-perf.ts`) that writes `test-results/canvas-perf.json`. Left other docs unchanged. |

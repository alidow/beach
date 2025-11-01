# Viewer Metrics & Stress-Test Plan

_Purpose_: define the instrumentation and automated coverage needed to guarantee “at-most-one WebRTC connect per tile” plus resilience under resize/reflow storms once the `SessionTileController` convergence lands.

This document is intentionally implementation-ready so a follow-up Codex instance (or teammate) can pick it up without additional clarification.

---

## 1. Metrics instrumentation

### 1.1. Emitters inside ViewerConnectionService
Add explicit counters whenever the service transitions between states. Suggested events (all logged through `emitTelemetry` + exported to a lightweight in-memory counter for tests):

| Event name | Trigger | Payload |
| ---------- | ------- | ------- |
| `viewer.connect.started` | `scheduleConnect()` before dialing transport | `{ tileId, sessionId, attempt, keyHash }` |
| `viewer.connect.success` | first time status becomes `connected` | `{ tileId, sessionId, attempt, latencyMs, secure: boolean }` |
| `viewer.connect.retry` | reconnect scheduled (after close/error) | `{ tileId, sessionId, attempt, delayMs, reason }` |
| `viewer.connect.failure` | connect throws before establishing transport | `{ tileId, sessionId, attempt, error }` |
| `viewer.connect.disposed` | controller releases tile (unmount or key change) | `{ tileId, sessionId, reason }` |

Implementation notes:
1. Extend `ViewerConnectionService` to accept an instrumentation interface (`ConnectionMetricsRecorder`). Provide a default recorder that logs to `console.info` + increments a global `WeakMap<string, TileMetrics>` for debugging.
2. Expose `viewerConnectionService.getCounters(tileId)` so tests can assert connect counts; counters should include: `started`, `completed`, `retries`, `failures`, `dispose`.
3. Ensure metrics survive hydration / tile re-render: the controller should feed the same `tileId` consistently so the counters aggregate across UI remounts.

### 1.2. Controller telemetry hooks
1. When the controller enqueues a measurement or applies host dimensions, emit `viewer.measurement.enqueue` with dedupe results (skipped, applied).
2. When controller triggers persistence, include aggregated metrics summary per tile (`connectStarted`, `connectCompleted`) to make debugging easier.

---

## 2. Test harness

### 2.1. Unit-level tests (Vitest)
Target file: `apps/private-beach/src/controllers/__tests__/viewerConnectionService.test.ts` (new).

Scenarios:
1. **Single connect** – Call `connectTile` twice with the same key. Assert recorder sees `started=1`, `completed=1`, no retries.
2. **Key change** – Connect with key `A`, then `B`. Assert service disposes old connection (`disposed=1`) and connects once for `B`.
3. **Retry path** – Mock `connectBrowserTransport` to throw once, succeed second time. Assert `started=2`, `completed=1`, `retries=1`.
4. **Disconnect cleanup** – Call `disconnectTile`, ensure counters record `disposed`.
5. **Snapshot emission** – Ensure subscribers get stable snapshot references when state doesn’t change (guards against re-render loops used in TileCanvas tests).

Use dependency injection / jest mock style:
```ts
vi.mock('../hooks/sessionTerminalManager', () => ({
  acquireTerminalConnection: vi.fn(),
  normalizeOverride: vi.fn(),
}));
```

### 2.2. Controller tests (Vitest)
Target file: `apps/private-beach/src/controllers/__tests__/sessionTileController.test.ts` (new).

Scenarios:
1. **Measurement dedupe** – enqueue identical measurement twice; expect `connectMetrics.started` unchanged, single snapshot update.
2. **Measurement storm** – enqueue 100 measurements quickly; ensure flush dedupes and metrics record applied count.
3. **Host priority** – host measurement should override DOM measurement signature.
4. **Persistence trigger** – apply layout update and confirm throttle schedules one persist call; check metrics summary included in payload.

### 2.3. Component-level tests
Extend `TileCanvas` test suite to inject mock controller + metrics recorder:
1. Render tile, trigger `onPreviewMeasurementsChange` multiple times → expect only one `viewer.measurement.enqueue` with `skipped` flagged appropriately.
2. Simulate toggle expand/collapse rapidly → metrics `connectStarted=1`, no extra connects.

### 2.4. Playwright “resize storm” test
New E2E spec: `apps/private-beach/tests/e2e/viewer-resize-stability.spec.ts`.

Steps:
1. Seed page with 1 tile.
2. Programmatically trigger a sequence of resizes via browser `window.resizeTo` or DOM style manipulations (10+).
3. Poll instrumentation endpoint (see §3) to assert `connectStarted=1`, `completed=1`, `retries=0`.
4. Optionally verify console logs contain `viewer.measurement.enqueue` with `skipped` counts.

---

## 3. Instrumentation surface for tests

To allow Playwright & Vitest to read counters without peeking into private state:

1. Create `apps/private-beach/src/controllers/metricsRegistry.ts`:
   ```ts
   export type ViewerTileCounters = {
     started: number;
     completed: number;
     retries: number;
     failures: number;
     disposed: number;
   };

  const counters = new Map<string, ViewerTileCounters>();

  export function increment(tileId: string, key: keyof ViewerTileCounters) { /* ... */ }
  export function getCounters(tileId: string): ViewerTileCounters | undefined { /* ... */ }
  export function resetCounters() { counters.clear(); }
  ```
2. Hook `ViewerConnectionService` to call `increment`.
3. In Next.js app (dev/test builds only), expose an endpoint `/api/debug/viewer-metrics` returning `Array<{ tileId, counters }>` for Playwright polling (guard behind `NODE_ENV !== 'production'`).
4. Provide helper for component tests: `import { resetCounters, getCounters } from '../controllers/metricsRegistry';`.

---

## 4. Implementation checklist

1. [ ] Add metrics recorder interface + default implementation.
2. [ ] Wire recorder into `ViewerConnectionService` events.
3. [ ] Export registry helpers for tests + Next API route.
4. [ ] Write Vitest unit tests (service + controller).
5. [ ] Extend TileCanvas tests to assert counters under measurement updates.
6. [ ] Add Playwright resize storm spec.
7. [ ] Update docs (`react-lifecycle-issues/overview.md`) with metric naming.

---

## 5. Hand-off prompt

```
You’re continuing the viewer metrics project.
Context: read docs/private-beach/react-lifecycle-issues/viewer-metrics-test-plan.md.

Implement Checklist items 1–3:
1. Add metrics recorder to ViewerConnectionService using the counters described.
2. Expose helpers in metricsRegistry.ts and integrate with the service/controller.
3. Create a debug API route /api/debug/viewer-metrics (dev/test only).

Ensure unit tests are updated/mocked accordingly. Run lint + existing tests afterwards.
```

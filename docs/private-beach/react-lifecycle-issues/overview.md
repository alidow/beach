# React Lifecycle Refactor: Session Tiles

## Symptoms we see today
- Measuring a tile (`[canvas-measure] apply …`) triggers `updateLayout('measurement', …)`, which returns a new layout object even when nothing changed. React-Flow sees a different node payload, tears down the existing preview, and mounts a fresh component.
- Every remount calls `useSessionTerminal` again, so the WebRTC viewer reconnects. The log excerpt from `temp/private-beach.log` shows the loop: `viewer-change → host-dimensions → terminal-hydrate session-miss → unmount → mount` repeating indefinitely.
- Because layout reconciliation and viewer connection are intertwined, innocuous events (DOM resize, viewport clamp) can cascade into the reconnection loop while the user is typing in the host session.

## Goals for the refactor
- Treat layout + viewer state as long-lived application data owned outside React.
- Guarantee at-most-one WebRTC connection per `(sessionId, transportConfig)` regardless of React renders, tab focus changes, or layout churn.
- Make layout updates deterministic: only apply mutations when the canonical state actually changes, and surface snapshots to React with stable identities.
- Reduce measurement complexity by funnelling DOM/host feedback through a single command queue that dedupes redundant updates before React ever sees them.
- Provide deterministic tests and instrumentation so regressions surface immediately (e.g., reconnect counters, simulated resize storms).

## Target architecture

1. **SessionTileController (client-side, framework-agnostic)**
   - Lives alongside React but outside the component tree (plain TypeScript module).
   - Holds canonical tile state: layout (position, dimensions, zoom), viewer snapshot, measurement metadata, and derived flags (loading/error).
   - Exposes a `useSyncExternalStore`-compatible subscription API so React components render readonly snapshots that only change when substantive data changes.
   - Accepts commands (`applyLayout`, `recordMeasurement`, `setHostDimensions`, `setViewerSnapshot`, …) and handles dedupe/ordering internally.

2. **ViewerConnectionService (state machine)**
   - A singleton service keyed by `(sessionId, transportConfig, auth)` that manages WebRTC lifecycle states: `idle → connecting → streaming → error → retry`.
   - Emits state changes to the controller; never recreates a connection unless the key changes.
   - Handles reconnect backoff, error handling, and metrics independent of React renders.

3. **Measurement & layout reconciliation pipeline**
- DOM resize observers enqueue measurement commands into the controller’s debounced queue keyed by tile id.
- The controller compares incoming measurements against canonical state (including sequence/version). Identical measurements are dropped synchronously; updated ones mutate state and emit a single snapshot.
- Host telemetry runs through the same queue but has precedence: measurement signatures encode the payload source, the active layout metadata records whether the last-applied update came from the host, and any DOM payload whose `measurementVersion` is <= the most recent host update is rejected before it reaches the queue. This guarantees that when a host packet and DOM observer share a version, the host dimensions remain authoritative for both layout metadata and downstream signatures.
- Preview callbacks now invoke `controller.applyHostDimensions` from both TileCanvas and CanvasSurface, reusing the emitted measurement objects so host rows/cols reach the controller queue even if DOM observers haven’t reported yet, while preserving signature stability.
- When host dimensions win, the resulting grid metadata, layout persistence, and telemetry exports all reflect the host-sourced measurement so downstream consumers never observe the transient DOM-only values.

4. **React layer (pure presentation)**
   - Components subscribe to controller snapshots and render them via React Flow.
   - Node data is derived from snapshots and memoised so React Flow receives stable references unless the snapshot hash changes.
   - No component contains side-effectful `useEffect` hooks for layout or connectivity; user actions dispatch commands back to the controller/service.

5. **Persistence & networking**
   - Layout persistence happens in the controller via throttled writes (e.g., after quiescence) so the API sees clean, deduped payloads.
   - Viewer connectivity continues to talk directly to Beach Road (no extra hop). The manager’s viewer remains separate for Redis caching.

## Execution plan

1. **Scaffold the controller + store**
   - Create `SessionTileController` responsible for tile registry, layout state, measurement queue, and viewer snapshot ingestion.
   - Implement a small observable/store that supports `subscribe()` and `getSnapshot()` compatible with `useSyncExternalStore`.
   - Port existing layout-fetch + persistence logic into controller commands (`bootstrapFromServer`, `persistLayout`).

2. **Introduce the viewer state machine**
   - Build `ViewerConnectionService` that maps connection keys to finite state machines.
   - Expose an observable per key and methods `connect(key)`, `disconnect(key)`, `forceRetry(key)`.
   - Update the controller to request connections based on tile lifecycle events and push resulting viewer snapshots back into tile state.

3. **Wire measurements through the controller**
   - Replace component-level measurement effects with `controller.enqueueMeasurement(tileId, measurementPayload)`.
   - Inside the controller maintain `lastMeasurementSignature` per tile; only mutate state if the signature changed.
   - Merge host telemetry via `controller.applyHostDimensions(tileId, hostDims)` with precedence rules.

4. **Refactor React components**
   - Replace existing hooks (`useSessionTerminal`, layout hooks) with `useTileSnapshot(tileId)` that reads from the controller.
   - Generate React Flow node data from snapshots using `useMemo` keyed by snapshot identity.
   - Ensure tiles never change `key`s across renders; the controller governs identity.

5. **Persistence + side effects**
   - Move layout persistence (POST/PUT to manager) into the controller, triggered by debounced canonical changes.
   - Ensure the controller can hydrate from server layout + Redis diff before React mounts, so the initial snapshot is ready on first render.

6. **Verification & observability**
   - Add counters/metrics (`viewer_connect_started`, `viewer_connect_completed`) keyed by tile to assert single connection per open tile.
   - Write integration tests that simulate: rapid resize events, layout drags, reconnect storms, and ensure the viewer state machine only enters `streaming` once.
   - Maintain opt-in logging for debugging but rely on metrics/tests for regression gating.

7. **Migration strategy**
   - Build the controller/service in parallel with the current implementation behind a feature flag.
   - Port a single tile path end-to-end, validate in staging, then roll out to all tiles.
   - Remove old hooks/effects once the controller-backed path is stable.

## Progress log

- 2025-03-02 — Scaffolded `SessionTileController` (useSyncExternalStore-compatible) and `ViewerConnectionService` singleton. Tile snapshots now capture layout, viewer state, cached diffs, and measurement signatures; connection manager dedupes connect/reconnect counts for upcoming metrics. React integration remains pending.
- 2025-03-03 — Hydrated the controller from `CanvasSurface`, replaced component-local layout state with `useCanvasSnapshot`/`useTileSnapshot`, and routed terminal measurements through `sessionTileController.enqueueMeasurement`. Tiles now render `SessionTerminalPreview` with controller-sourced viewer snapshots, while layout mutations flow back via controller commands.
- 2025-03-03 — Session previews now require controller-sourced viewer snapshots (`SessionTerminalPreviewClient` no longer calls `useSessionTerminal`). `TileCanvas` hydrates the controller for grid tiles, consumes `useTileSnapshot`, and forwards measurement updates through the controller queue; the legacy viewer cache remains only to drive the expanded overlay UI.

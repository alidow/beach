# Private Beach Canvas — Terminal Preview & Host Resize Integration

_Owner: Codex instance focused on `SessionTerminalPreviewClient` + `BeachTerminal` updates. Keep the progress log current._

## Objective
Deliver the refined driver/clone terminal preview pipeline for the canvas, including explicit host resize controls, measurement versioning, and performance throttling so tiles stay compact and accurate without regressing latency or UX.

## Scope
- SessionTerminalPreviewClient (`apps/private-beach/src/components/SessionTerminalPreviewClient.tsx`)
  - Ensure off-screen driver uses `disableViewportMeasurements`, `contain:size`, and no longer influences DOM measurements.
  - Maintain the visible clone wrapper sized to `targetWidth/Height` with `transform: scale(...)`.
  - Track `measurementVersion` so stale measurements are ignored when host metadata changes mid-flight.
  - Emit measurement payloads (`targetWidth`, `targetHeight`, `scale`, `hostRows`, `hostCols`, `measurementVersion`) to the canvas state.
  - Throttle preview rendering / requestAnimationFrame work for unfocused or off-screen tiles (likely via `IntersectionObserver` + reduced FPS).
  - Provide lifecycle hooks for the canvas surface to know when previews are ready, reconnecting, or errored.
- BeachTerminal (`apps/beach-surfer/src/components/BeachTerminal.tsx`)
  - Expose a new `requestHostResize({ rows, cols })` API that bypasses `lastMeasuredViewportRows` and uses explicit inputs.
  - Ensure existing resize flows remain intact for other consumers (Cabana/CLI) — guard the new behaviour behind props.
  - Prevent zero-height (1 row) resize loops when the driver is hidden.
- Host resize orchestration
  - Update locked/snap logic to compute desired `rows/cols` from visible clone size and invoke the new API.
  - Debounce resize requests (avoid bursts when host already resizing) and log diagnostic events (`[terminal][resize] ...`).
- Performance & diagnostics
  - Maintain `window.__PRIVATE_BEACH_DEBUG__` logging (or similar) with updated payloads.
  - Add measurement timing instrumentation to confirm the driver and clone stay in sync.
  - Document recommended profiling steps (e.g., Chrome Performance, React Profiler).

## Dependencies & Coordination
- Coordinate response formats with the canvas surface track (`canvas-surface-implementation.md`) so measurement updates bind cleanly.
- Inform the backend track if any additional metadata must be persisted (e.g., content scale).
- Work with the testing/performance track to expose programmatic hooks or flags to drive automated validation.

## Deliverables Checklist
- [x] SessionTerminalPreviewClient updated with measurement versioning and throttling.
- [x] BeachTerminal exposes `requestHostResize` (documented props/type defs).
- [x] Locked tile flow uses explicit rows/cols rather than driver measurements.
- [x] Debug logging and diagnostics refreshed.
- [x] Manual verification notes + automated coverage (unit or integration) added *(BeachTerminal.requestResize.test.tsx covers requestHostResize clamping; broader canvas e2e coverage still owned by testing track).* 

## Verification Steps (start filling in as you implement)
1. ✅ `cd apps/beach-surfer && npx vitest run src/components/BeachTerminal.requestResize.test.tsx` — verifies `requestHostResize` clamps rows (≥2, ≤512) and honors explicit cols.
2. ⏳ Local Storybook/preview demonstrating stable sizing after host resize *(pending manual walkthrough).* 
3. ⏳ Performance profiling with 20+ tiles to confirm off-screen throttling benefits *(pending performance track).* 

## Progress Log
_Append updates chronologically._

| Date (YYYY-MM-DD) | Initials | Update |
| ----------------- | -------- | ------ |
| 2025-10-30 | CA | Reviewed integration doc + remaining phases; inspected Surfer/Private Beach logs to align API changes. |
| 2025-10-30 | CA | BeachTerminal: added requestHostResize({rows, cols}), clamped min rows≥2, preserved existing sendHostResize, and introduced optional maxRenderFps to throttle rAF-based updates. |
| 2025-10-30 | CA | SessionTerminalPreviewClient: switched driver to disableViewportMeasurements with contain:size; added IntersectionObserver-based visibility and throttled clone rendering; implemented measurementVersion and expanded preview payload (hostRows/hostCols/version). |
| 2025-10-30 | CA | TileCanvas: wired explicit host resize orchestration (compute rows/cols from visible tile), debounced requests, added diagnostic logs, and guarded against stale preview updates via measurementVersion. |
| 2025-10-30 | CA | Added `BeachTerminal.requestResize.test.tsx` coverage for explicit resize API and documented verification steps in this plan. |

# Terminal Preview Tile Height – Diagnosis & Refactor Plan

_Last updated: 2025-10-30_

## 1. Background

- **Surface:** Private Beach dashboard (`apps/private-beach/src/pages/beaches/[id]/index.tsx`)
- **Preview component:** `SessionTerminalPreviewClient`
- **Transport:** Beach Surfer WebRTC client (`apps/beach-surfer`)
- **Hook (legacy):** `useSessionTerminal` (now replaced by controller-driven `viewerConnectionService` subscriptions)
- **Current behaviour:** The visible terminal preview tile only shows ~24 rows (upper half). The underlying PTY is 62×104, but the preview never expands to the full height even after the connection stabilises.

### Key Observations

1. Tile dimensions in React Flow update (CanvasSurface logs `metadata-only`, `size-update`), so Canvas is receiving measurement events.
2. The hidden “driver” `<BeachTerminal>` (used for off-screen measurement) always logs `disable-measurements viewport-set … appliedRows: 24`. Because `disableViewportMeasurements` is true, the driver never runs its ResizeObserver and never reports real host viewport rows.
3. Every `emit-viewport-state` payload has `hostViewportRows: null`. Our `SessionTerminalPreviewClient` hook falls back to the measured viewport height (24) whenever host rows are null, which clobbers any inferred PTY dimensions.
4. The DOM clone does render at the full raw pixel size (~464px), but since the hook resets `hostDimensions.rows` to 24 immediately afterwards, the terminal store clamps the viewport back to 24 rows on the next render.

## 2. Root Cause

The preview pipeline conflates two responsibilities:

- Measuring the PTY viewport via a hidden `<BeachTerminal>` instance.
- Rendering the visible clone for the user.

Both instances currently use `disableViewportMeasurements`, so neither runs the real measurement logic. As a result, the hook never receives true PTY dimensions and falls back to `viewportRows` (24) every cycle. Any attempt to infer rows/cols from the DOM is overwritten on the next `viewport-state` payload.

## 3. Proposed Refactor

### Goals

1. Let the measuring terminal gather authoritative PTY dimensions and emit them via `hostViewportRows` / `hostCols`.
2. Keep the visible clone passive (no resize side effects) to avoid loops.
3. Ensure `SessionTerminalPreviewClient` only falls back to viewport-derived numbers when we truly haven’t received PTY data.
4. Keep Canvas tile sizing in sync with the real PTY dimensions.

### Detailed Steps

#### Step 1 – Split Measurement Mode vs Display Mode

- Introduce an explicit `mode` prop (e.g. `{ mode: 'measure' | 'display' }`) for `<BeachTerminal>` usage within `SessionTerminalPreviewClient`.
- For the **measure** instance:
  - `disableViewportMeasurements={false}` (re-enable ResizeObserver).
  - `autoResizeHostOnViewportChange={false}` (don’t spam host resize requests from the measuring terminal).
  - `hideIdlePlaceholder`, `showTopBar={false}`, etc., remain as today.
- For the **display** instance:
  - `disableViewportMeasurements={true}` (avoid multiple observers).
  - `autoResizeHostOnViewportChange={locked}` (current behaviour).

Implementation approach:

- Extract the “driver” into its own component (e.g. `<TerminalPreviewDriver>`).
- Ensure it mounts once per session and persists across clone rerenders to avoid Strict Mode double-mount churn.

#### Step 2 – Host Dimension Handling

Update `SessionTerminalPreviewClient`’s `setHostDimensions` logic:

- When `hostViewportRows` / `hostCols` arrive (non-null), treat them as authoritative and persist them.
- Only fall back to viewport measurements when we have never observed PTY dimensions.
- Remove the DOM-based inference or keep it as a safety net but **gate** it so it doesn’t fight with real PTY data.

Add logging to confirm:

- `hostViewportRows` transitions from null → 62.
- `host-dimension-update` logs show the jump to 62 persists across reconnects.

#### Step 3 – Canvas Measurement Payload

- Continue to send `rawWidth` / `rawHeight` plus scale data to Canvas.
- Ensure `CanvasSurface` uses `rawWidth` / `rawHeight` for tile size (already switched) and retains `scale` for metadata.
- When tiles load from saved layouts, hydrate with existing PTY metadata (if present) to avoid resizing down to defaults before measurements land.

#### Step 4 – Testing / Verification

1. Unit tests:
   - Add a hook-level test (using `@testing-library/react-hooks`) that feeds synthetic viewport events (`hostViewportRows` vs null) and asserts `hostDimensions` behaviour (authoritative versus fallback).
   - Add a Canvas measurement test that verifies we don’t issue `metadata-only` when PTY data changes.
2. Manual (or E2E) validation:
   - Run `/dev/private-beach-sandbox` with a long PTY (fixture or live session) and confirm the preview shows full rows.
   - Resize/drag tiles, reconnect sessions, confirm tile height stays correct.

**Verification 2025-10-30:** `npm run test:e2e -- tests/e2e/private-beach-sandbox.spec.ts` now passes, and the sandbox fixture renders the full PTY banner (`PRIVATE BEACH PONG`).

### Operational Notes

- Existing logs (`temp/private-beach.log`) are very helpful; keep the instrumentation until fixes verified.
- The current measuring approach is brittle; consider a long-term plan to expose PTY stats directly through the transport (e.g. add host rows/cols to the data channel handshake).

## 4. Prompt for the Next Engineer

```
You’re taking over the private beach terminal preview issue. Current symptoms: preview tile only shows ~24 rows (upper half) even though PTY is >60 rows. See doc docs/private-beach/tile-sizing-issues/terminal-preview-refactor.md for full context.

Tasks:
1. Refactor SessionTerminalPreviewClient so the hidden <BeachTerminal> runs with disableViewportMeasurements=false and emits real hostViewportRows/hostCols. The visible clone should remain passive.
2. Adjust the host-dimension sync logic so it preserves PTY dimensions once received; only use viewport fallbacks when no PTY data exists.
3. Ensure CanvasSurface respects the raw PTY size (tile width/height match the real terminal dimensions).
4. Add targeted tests (or at least a hook-level unit test) to confirm hostDimensions update when hostViewportRows arrives.

After code changes, run the sandbox repro (/dev/private-beach-sandbox with a tall fixture) and confirm the entire terminal renders.

Deliverables: updated TypeScript files, tests if feasible, and a brief verification note in the doc once resolved.
```

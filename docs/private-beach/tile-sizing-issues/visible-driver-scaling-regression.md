# Visible Driver Scaling Regression – 2025-11-03

## Snapshot
- Environment: `repro/prev-commit` (commit `5ff3671`) is the "clean" baseline that almost works. Tail rows missing but tile aspect is correct.
- Current branch: `repro/prev-commit` with experimental changes layered on top. Harness flag `NEXT_PUBLIC_PRIVATE_BEACH_VISIBLE_DRIVER=true`.
- Session example: `5c78c5bd-aff5-4b85-9bbb-782f44da2231` (screenshot/logs in `temp/private-beach.log`).

## Current Behaviour
- Tile renders with correct width but visible driver canvas is cropped vertically. Sometimes the cropped canvas is “smooshed” into a narrow strip at the top; other times the tile height stays fixed at ~440 px and the bottom half of the PTY is clipped.
- In the latest log (`temp/private-beach.log`):
  - `preview-measurements … rawHeight: 1086, targetHeight: 720, scale ≈ 0.663` — preview expects a tall canvas.
  - `dom-dimensions … wrapperHeight: 440` — outer wrapper never grows beyond the initial 24-row measurement.
  - `disable-measurements viewport-set … forcedViewportRows: 62` — the visible driver correctly jumps to the real PTY height.
  - Result: the driver renders 62 rows (≈1086 px), the wrapper clamps at 440 px, so only the top strip shows.

## What We Tried (and Why It Failed)

1. **DOM scaling gate toggles** – allowed DOM measurements before PTY geometry, then forced host estimate afterward. This kept width stable but reintroduced the cropped bottom because the wrapper never re-sized after PTY lock.
2. **Geometry locking and swap correction** – successful at preventing PTY churn (rows/cols now lock at 62×106), but we still clamp the wrapper’s `targetHeight` to the original 440 px.
3. **Driver container sizing** – switched to measured line height + guard in `BeachTerminal.tsx`. This fixed missing tail rows within the driver but magnified the preview mismatch when the wrapper stayed small.
4. **Clone inner transform-only path** – attempted to drop explicit `width/height` for the visible driver. Without a matching change to the wrapper, the outer container still clipped the driver; in some runs the preview shrank back to 440 px because the DOM scale gate had not flipped yet.

## Diagnosis
- The wrapper (`cloneWrapperStyle`) still applies `targetHeight` computed from outdated `previewMeasurements`. Once PTY rows lock at 62, the driver publishes a taller canvas (1086 px). If the preview does not recompute `targetHeight` (because DOM scale gate never flipped or committed sample was 0), the wrapper remains locked to ~440 px → crop.
- In the latest log the DOM sampler returned `childWidth: 0, childHeight: 0` initially (visible driver still loading). Because `domCommittedSampleRef` never latched a non-zero value before PTY lock, `allowDomForScaling` remained false, so `rawHeight` stayed 440. Later, DOM reported `childHeight: 364` (still smaller due to wrapper clamp). Preview kept scaling off 440, so the wrapper target never increased.

## Proposed Next Steps

1. **Ensure DOM scale handshake completes**
   - Capture the first non-zero DOM measurement for the visible driver and mark it committed before PTY lock switches to 62 rows. If the first sample is zero, retain host estimate but retry until a non-zero sample arrives.
   - When a committed sample exists, recompute `previewMeasurements` to produce the new `targetHeight` (≈720). This should resize the wrapper.

2. **Force wrapper recalculation once geometry locks**
   - After `host-geometry-locked`, explicitly invalidate `previewMeasurements` (bump `domRawVersion` or `measurementVersion`) so the wrapper picks up the new raw canvas.
   - Optionally, set a minimum `targetHeight` based on `lockedHostRows * measuredLineHeight` even before DOM sample, to avoid waiting on DOM when PTY rows are known.

3. **Stabilize clone inner layout**
   - Visible driver should supply intrinsic size; only apply `transform`. For hidden clone, keep explicit width/height.
   - Verify that `cloneWrapperStyle` uses `previewMeasurements.targetHeight` derived from the new raw canvas.

4. **Validation**
   - Reload tile and confirm logs show `preview-measurements … targetHeight: 720`, `dom-dimensions … childHeight: 720`. No `[terminal][trace] host-dimensions-swap-corrected` spam after lock.
   - visually confirm full PTY (no crop) and no vertical scrollbar.

## Reference Commit
- Clean baseline (almost correct but missing tail rows): `5ff3671` on `main`. Revisit if regression needs bisecting.

## 2025-11-03 Implementation Notes

- Updated `SessionTerminalPreviewClient` to wait for a non-zero DOM canvas before enabling DOM scaling and to keep wrapper size in sync with either the committed DOM canvas or the host-derived PTY geometry (whichever is larger).
- When PTY geometry locks we now invalidate preview measurements so the wrapper can reflow immediately instead of waiting for a later DOM sample.
- Added a small CDP helper (`scripts/cdp-read-console.js`) that reloads a tile, tails console output, and can be used to capture the `preview-measurements` / `dom-dimensions` traces.
- Local verification is blocked at the moment because the `09e1fc4f-8922-4b51-9d91-d838e640c146` session shows `Connected - waiting for host approval…`, so the terminal never streams full geometry. Once the host approves, re-run `node scripts/cdp-read-console.js --match "09e1fc4f-8922-4b51-9d91-d838e640c146" --reload --duration 40` and confirm the wrapper grows to the locked PTY height and no vertical scrollbar appears.

# Private Beach Terminal Tile – Height vs Cropping Deadlock (October 2025)

## 1. Problem Summary

The Private Beach dashboard tile now auto-sizes and keeps WebRTC stable, but the terminal preview is still wrong:

- If we size for the host’s full PTY (104×62) the tile is extremely tall but content is centered correctly.
- As soon as we limit the tile to the actual viewport (24 rows) the terminal content crops to the upper-left quadrant.
- We need a bounded, non-cropped tile (≈450×450) that adapts to the real viewport without restarting the viewer.

## 2. Current Behaviour (as of commit after 2025-10-29)

- Tile layout uses a 128-column grid with 12px row height. Autosize clamps to 450px width/height caps.
- Zoom is computed from `measurements` vs `estimateHostSize`, factoring in viewport dimensions where available.
- The tile stays around 306×450 when the viewport reports 24 rows; zoom ≈ 0.35–0.51 depending on data source.
- Despite the smaller size, `SessionTerminalPreviewClient` still renders at the larger PTY size; only the top-left portion is visible.

## 3. Artifacts from the latest repro (`temp/private-beach.log`)

- `[tile-diag] autosize-apply` shows we save `widthPx: 306, heightPx: 450` (derived from grid units) after clamping.
- `[terminal][diag] host-dimensions` still logs `hostViewportRows: 62` even though `viewportRows: 24`. We clamp these to 24 before saving, but subsequent payloads reintroduce 62.
- `[tile-layout] tile-zoom` ends at ~0.347; `[terminal] target-size` uses the clamped measurement, but the viewer content is still cropped.

## 4. Work Done So Far

| Phase | Change | Outcome |
| --- | --- | --- |
| 1 | Added autosizing to 128-col grid, figured out WebRTC loop, introduced diagnostic logging | Stabilised layout/zoom but content still cropped once tile height reduced |
| 2 | Adjusted measurement storage to match grid units; zoom now reflects 24-row viewport, but preview still crops | Terminal rendering still limited to top-left corner |
| 3 | Clamped host rows/cols to viewport in `SessionTerminalPreviewClient` | Prevented giant tile, but crop reappeared |
| 4 | Reverted clamp to avoid crop, then stored normalized width/height; still cropped | No improvement – indicates root cause elsewhere |
| 5 | Removed `key={layoutSignature}` so tiles would not unmount, added more logging | Layout stable, preview still cropped when tile shrinks |

## 5. Where We’re Stuck

1. **Viewer still renders at the larger PTY size.** Even when we cap `hostRows` to 24, BeachTerminal appears to keep a 62-row framebuffer and the viewer scales content as if it were larger.
2. **Viewport payload inconsistency.** Logs show `hostViewportRows: 62` immediately after we set it to 24. Need to trace where that 62 comes from (likely from the host after a PTY resize).
3. **Measurement mismatch.** `target-size` events show `targetHeight: 450`, so the client thinks the preview should be 450px tall. Actual DOM height is ~450, but content is cropped. Need to inspect BeachTerminal’s scaling logic.

## 6. Suggested Next Investigation Steps

1. Instrument `BeachTerminal.tsx` to log its internal `computedRows/cols`, cell metrics, and the CSS transform applied to the canvas after we receive host viewport updates.
2. Verify whether xterm/terminal rows are still measured from the old PTY (62). If so, we may need to send an explicit resize to the host or force the viewport to use capped values for measurement.
3. Inspect the `targetSize`/`scale` combination. If we pass a height larger than viewport content, xterm may clip. Try setting `targetHeight` based on the viewport rows (not the grid-derived height) to confirm the hypothesis.
4. If BeachTerminal caches the host PTY (62) independently, we may need to update the viewer API to include the clamped viewport rows or send a manual `setViewportSize` call.




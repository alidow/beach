## Private Beach Tile Sizing – Investigation Notes (April 2025)

### 1. Snapshot  
- **Component(s)**: `TileCanvas`, `SessionTerminalPreviewClient`, `BeachTerminal` (xterm wrapper)  
- **Primary symptom**: Terminal preview inside a 3×3 tile renders as a single compressed row that briefly flashes at full size during load.  
- **Layout mismatch**: React Grid Layout item is `w=3`, `h=3`, DOM box ~`624×362`, yet terminal content scales down to ~0.17 and collapses vertically.  
- **PTTY reality**: Host-side terminal is ~62 rows high (Pong LHS demo); xterm viewport events oscillate between 40–110 rows, never reporting the real PTY dimensions.

### 2. Observability & Artifacts  
- **Console log dump**: `temp/private-beach.log` (copy raw DevTools console into this file).  
- **DOM snapshot**: `temp/dom.txt` (full HTML capture of `.session-grid-item`).  
- **Auxiliary logs**:  
  - `[tile-layout] tile-zoom` – logged from `TileCanvas.tsx`, includes `hostRows`, `viewportRows`, `zoom`.  
  - `[tile-layout] viewport-payload` – raw payload arriving from `SessionTerminalPreview`.  
  - `[terminal] viewport-state` – xterm viewport instrumentation (`viewportRows`, `hostViewportRows`, `hostCols`).  
  - `[terminal] zoom-wrapper` & `[terminal] target-size` – preview wrapper scale + the measured tile dimensions.

### 3. Repro Steps  
1. Run Private Beach dashboard (locally: `pnpm dev --filter private-beach`).  
2. Load `/beaches/<PRIVATE_BEACH_ID>` in browser (Clerk dev key).  
3. Ensure a session tile is present (the Pong LHS demo).  
4. Observe tile width (about 620 px) but terminal content collapsed to the first row.  
5. Check DevTools console → copy to `temp/private-beach.log`.  
6. Optionally capture DOM → `temp/dom.txt` (see instrumentation docs for snippet).  
7. Resize tile or reload – behaviour persists; zoom drops to ~0.17 as soon as viewportRows bounces.

### 4. Investigation Timeline (abridged)  
| Time | Action | Outcome |
| --- | --- | --- |
| T0 | Confirmed original bug: grid layout width vs DOM width mismatch | tiles locked to 3 columns but preview stretched full width |
| T1 | Added layout signature logging + `targetSize` measurement (ResizeObserver) | Confirmed tile measured width ≈ 547.5 px used for zoom |
| T2 | Added `zoom-wrapper` instrumentation (`SessionTerminalPreviewClient`) | Observed wrapper scaling to 0.6→0.16; height px derived from hostRows |
| T3 | Tweaked computed zoom & min zoom floor | Prevented total collapse but still cropped |
| T4 | Fallback host rows derived from viewportRows | Host rows bounced between 40–110 → zoom oscillation |
| T5 | Attempted monotonic fallback | Still locked to lower bound (43) → compressed viewport |

Full console history is preserved in `temp/private-beach.log`.

### 5. Key Findings  
- **No authoritative host rows**: `TerminalViewportState.hostViewportRows` remains `null` throughout connection.  
- **xterm viewport jitter**: `viewportRows` increases as buffered rows accumulate (43, 66, 107, 137, …).  
- **Scale derives from hostRows**: `TileCanvas.computeZoomForSize` divides tile measurements by `estimateHostSize(hostCols, hostRows)`; with hostRows ≈ 40, scale → 0.16.  
- **Wrapper height**: Terminal wrapper uses `transform: scale(...)` with base height computed from `hostRows`; when height is underestimated, the scaled content collapses vertically.  
- **React-Grid width** is correct; issue is purely vertical scaling vs PTY size.  
- **Host reality** (~62 rows) never reaches the client – without this, zoom remains misaligned.  
- **Logging** proves that even after fallback/clamping changes, hostRows never exceeds low 40s because each viewport payload resets the estimate.
- **Preview feedback loop (2025-04 follow-up)**: After introducing the wrapper scale, `BeachTerminal`’s ResizeObserver measures the already-scaled row height (~0.11 px) and writes it back to CSS variables, which collapses the DOM again. The fix needs to break this loop so the base line height stays near 17 px while only the wrapper handles scaling.

### 6. Working Hypothesis  
> _The terminal preview relies on accurate PTY rows/columns, but the session bridge only streams the current viewport size. Without the true host rows, the client downscales using a much smaller target, so the rendered terminal height is compressed. Extracting the host PTY dimensions (from the manager or the bridge handshake) and sending them alongside the stream would allow the tile to compute a stable zoom._

### 7. Proposed Fix Approach  
1. **Source host PTY size**  
   - Extend manager/session metadata to include `host_rows`, `host_cols` (e.g., via `/sessions/:id` or the bridge handshake).  
   - Ensure `SessionTerminalPreviewClient` receives those values (prop via `TileCanvas` → `SessionTile`).  
2. **Update preview component**  
   - When host rows/cols are provided, stop inferring from viewport events.  
   - Only use viewportRows for zoom when hostRows is `null` and never let it shrink once a host value is set.  
   - Disable `BeachTerminal`’s viewport measurement loop when the preview is running inside a scaled wrapper to preserve the authoritative line height.  
3. **Clean up fallback heuristics**  
   - Remove the rolling clamps that try to guess from viewport data once real host rows are available.  
   - Keep logging but downgrade noisy payload output once behaviour stabilises.  
4. **Verification**  
   - Re-run Pong session; tile should display full terminal at ~60 rows without animation “bounce”.  
   - Resize tile → zoom recalculation should be smooth, no cropping or single-row collapse.

### 8. Additional Notes  
- Previous docs: `docs/private-beach/tile-sizing-issues/analysis.md` & `status-2025-03-xx.md` chronicled earlier width bug; this document supersedes them for vertical scaling specifics.  
- Any future agent should pull fresh logs after implementing fixes to confirm `[tile-layout] tile-zoom` hostRows matches real PTY rows.  
- If bridge changes are invasive, consider an interim hack: allow operators to manually override host rows for a tile (not ideal, but unblocks demos).

### 9. April 2025 Fix Summary
- `BeachTerminal` now falls back to the grid frame’s `historyRows` when `viewportRows` is omitted, preserving the host PTY height even when the bridge omits it.
- `SessionTerminalPreviewClient` caches the first authoritative PTY dimensions it sees (from either host metadata or the initial viewport measurement) and reuses them for scaling and the `viewport-dims` callback. This prevents jitter and guarantees `TileCanvas` always receives consistent `hostRows`/`hostCols`.
- `TileCanvas` simplifies its host-dimension handling: once a host size is known it is treated as authoritative, and all of the previous monotonic viewport heuristics are removed.
- `BeachTerminal` learned to opt out of ResizeObserver measurements when instructed (preview mode), which stops the transform-induced feedback loop and preserves the ~17 px base line height.  
- `TileCanvas` now bumps freshly-added terminal tiles to a host-aware footprint (still compact, but tall enough for 60+ rows) the moment real PTY dimensions arrive, so zoom stays consistent without the huge right-side gutter.
- With the new data flow and measurement guard, the terminal preview renders the full ~62 host rows at a stable zoom (~0.25) instead of collapsing to a single row after viewport jitter.
- Instrumentation remains in place; expect `[terminal] viewport-dims dispatch` to show `hostRows` ≈ `62` and `[tile-layout] tile-zoom` to log `hostRows` consistently for the Pong demo tile.

---

_Last updated: 2025-04-XX (Arelli / Codex fix pass)_

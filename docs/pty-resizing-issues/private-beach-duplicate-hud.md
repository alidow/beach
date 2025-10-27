# Private Beach Tile Resizing – HUD Duplication Investigation

## Context

- **Symptom**: After increasing the height of a Private Beach tile that hosts the Pong demo TUI, the top of the BeachTerminal viewport shows duplicated HUD / prompt lines instead of blank space.
- **Expectation**: Newly exposed rows should remain blank (i.e. render `MissingRow`) until the PTY actually sends fresh content for those positions.
- **Observation**: The duplication appears every time the tile grows, even though the same terminal content in a local host shell does not show the extra HUD block.

## Timeline & What We Tried

1. **Initial hypothesis** – `visibleRows()` in `apps/beach-surfer/src/terminal/cache.ts` was recycling old rows when the viewport requested more height.  
   - Implemented tail padding so that newly visible rows are returned as `MissingRow`.  
   - Added regression test (“pads newly exposed tail rows…”) and manually verified padding via traces.

2. **Problem persisted** – Real tiles still showed the duplicated HUD.  
   - Added trace instrumentation for `visibleRows`, `buildLines`, and the follow-tail toggle.  
   - Learned that the viewer was momentarily flipping out of follow-tail mode during resize, which bypassed the padding path.

3. **Second fix attempt** – Preserve tail padding even after follow-tail is disabled.  
   - Cache now stores a `tailPadRange` + `tailPadSeqThreshold` and applies it in both tail and scrollback paths.  
   - Added test (“keeps padding active after followTail is disabled”) to cover this path.

4. **Still no joy** – Tile continued to display the duplicate HUD.  
   - Latest traces show `visibleRows tail (padded)` firing correctly (e.g. `rowKinds` starting with six `MissingRow` entries).  
   - Immediately afterward, the server sends a `snapshot/history_backfill` replay that rewrites those same rows with higher `seq` values containing the HUD text (`Unknown command`, `Commands`, `Mode`, `>`).  
   - Because the rows are authoritative and newer than the padding threshold, the cache correctly promotes them back to `LoadedRow`.

5. **Latest fix** – Treat authoritative `history_backfill`/`snapshot` replays differently while tail padding is active.  
   - Added a `window.__BEACH_TRACE_DUMP_ROWS` hook (plus `window.__BEACH_TRACE_LAST_ROWS`) so we can capture `visibleRows` payloads as JSON directly from the browser console.  
   - Reproduced the resize → replay flow in `cache.test.ts` (see “keeps tail padding when authoritative backfill replays existing rows” and “keeps tail padding when delta replays identical rows”).  
   - Updated `TerminalGridCache` to treat tail-pad rows as immutable until new content arrives: redundant `history_backfill` *and* `delta` frames—whether they reuse the original `seq` or bump it—are skipped if the text is unchanged, so those rows stay `MissingRow`. Tail padding now remains active even if the viewport briefly shrinks after a resize (e.g. when the host sends a smaller `viewportRows` in a grid frame).  
   - All existing tests still pass and the new regressions keep the padded rows blank.

## Current Understanding

- The duplication is triggered by `snapshot` / `history_backfill` *and* immediate `delta` frames that re-send the HUD rows right after the resize—sometimes with higher `seq` values, other times reusing the original sequence numbers. Tail padding now ignores those redundant redraws so the newly exposed rows stay `MissingRow` until fresh content actually changes the text.
- `window.__BEACH_TRACE_DUMP_ROWS` makes it easy to capture the viewer’s perspective (absolute, seq, text) right after resize for future debugging.
- `cache.test.ts` now covers the resize → authoritative replay sequence, preventing regressions in the padding logic.
- The upstream cause (why the host pipeline replays the HUD after resize) is still open, but it no longer breaks the UI and can be investigated separately.

## Open Questions

1. **Should we still chase the upstream cause?**  
   - The fix keeps the client clean, but we may want to gather a PTY byte dump during resize to confirm whether the emulator/backfill path is replaying stale HUD content.

2. **Does any other surface rely on authoritative backfill when following tail?**  
   - If we ever expect legitimate `snapshot` frames to populate newly padded rows while following tail, we should validate those flows against the new guard.

3. **Telemetry / logging follow-up**  
   - Decide whether the trace hook output should be captured in a shared location (e.g., attach to incidents) or left as an on-demand debugging tool.

## Next Steps (Plan)

1. ✅ Viewer instrumentation + regression test + cache guard are all merged (`TerminalGridCache` now ignores authoritative backfill rows while tail padding is active).
2. 🔜 Validate in a real Private Beach session and grab a `window.__BEACH_TRACE_DUMP_ROWS()` capture (check the JSON output or `window.__BEACH_TRACE_LAST_ROWS`) so we can confirm the HUD stays padded and inspect the row text/seqs.
3. 🔜 If the host continues to replay HUD bytes, collect a PTY byte stream and loop in the PTY/emulator owners for a deeper fix.
4. 📓 Keep this document updated with any real-world verification notes or follow-up tickets.

## How to Proceed

To pick up from here, a new agent should:

1. Reproduce the resize in a real Private Beach tile with tracing enabled (`window.__BEACH_TRACE = true`), then run `window.__BEACH_TRACE_DUMP_ROWS()` immediately after the resize to capture the viewer state.
2. If the HUD still reappears, snag the PTY byte stream (or request it from the host side) so we can debug the upstream replay.
3. Share any captures + notes here and with the PTY/emulator owners; open a follow-up ticket if host-side work is required.

Please keep this document in sync with any new discoveries so we have a clear, collaborative record.

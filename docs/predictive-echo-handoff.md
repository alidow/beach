# Predictive Echo Space Drift — Handoff Notes

## Current Symptom

- Predictive overlay drifts right when typing quickly at the prompt, most visible once a space is entered after fast keystrokes.  The analyzer still reports mismatches such as `seq 5 row=0 col=31 mismatch` and repeated "acked but predictions never cleared" rows.
- Behaviour reproduces reliably in the Singapore test session while running the Rust TUI (`beach-human`).

## Reproduction / "Litmus" Test

```bash
rm -f /tmp/beach-debug.log && \
  BEACH_LOG_FILTER=debug,client::predictive=trace cargo run -p beach-human -- \
    --log-level trace \
    --log-file /tmp/beach-debug.log \
    ssh --ssh-flag=-i --ssh-flag=/Users/arellidow/.ssh/beach-test-singapore.pem \
        ec2-user@54.169.75.185
```

1. Once connected, type quickly at the shell prompt; use sequences like `echo "hello world"` followed immediately by ` <space>` to trigger the drift.
2. Observe double characters in the TUI and mismatched cursor alignment.

## Log Analysis Workflow

1. Collect logs with the command above (`/tmp/beach-debug.log`).
2. Run the predictive analyzer for detailed sequence diagnostics:
   ```bash
   python scripts/analyze-predictive-trace.py /tmp/beach-debug.log --verbose
   ```
3. Key red flags still present after recent fixes:
   - `server content did not match predictions` on row 0/row 2 immediately after a space.
   - `acked but predictions never cleared`, typically for the space sequence (`seq 7` / `seq 8`).
   - `acknowledged/cleared without a registration` for follow-on sequences once overlays have been rebased.
4. Correlate the above with `prediction_registered`, `prediction_dropped`, and `prediction_update_overlap` events in the raw log (`rg 'prediction_' /tmp/beach-debug.log`).

## Work Completed This Session

- Added dropped-prediction tracking to capture overlays cleared before ACKs and surfaced the reason (`apps/beach/src/client/terminal.rs`).
- Synced renderer and pending prediction lifecycles, ensuring trims, overlaps, and overlay pruning update both structures.
- Reworked ACK handling to log drop dwell time, outstanding sequence ranges, and RTT samples even when predictions disappear early.
- Updated cursor handling so predicted cursor positions are always advanced (no longer gated on authoritative frames) and `sync_renderer_cursor` clamps only where needed.
- Introduced `rebase_predictions_for_row`/`shift_predictions_left` to shift pending overlays when the server resets the cursor (e.g., prompt rewrite).
- Added regression tests:
  - `predictive_server_overlap_moves_prediction_to_drop_queue`
  - `predictive_cursor_flushes_predictions_when_authoritative_pending`

All tests in `cargo test -p beach-human` targeting the new cases pass, but the live SSH trace still reproduces the mismatch.

## Outstanding Observations

- Analyzer shows the very first predictive space on the prompt (`seq 5`) mismatching the server's authoritative `c`, suggesting the server rewrites the prompt immediately after we send the space. Our client now rebases predictions when the cursor jumps left post-handshake, but the mismatch persists because the renderer still clears the space before the authoritative prompt redraw lands.
- Predictions for subsequent characters on row 1/row 2 (after newline) also mismatch, implying our cursor row advance may still be ahead of the server when the newline is processed.
- Dropped-prediction logs confirm we register and then immediately drop the space (`reason=renderer_trim`) before the ACK, causing the analyzer to treat the following authoritative write as a mismatch.

## Suggested Next Steps

1. Capture synchronized host logs (or a screen recording) to confirm the server redraw order around the space/newline boundary.
2. Instrument the client to record the renderer's committed row width before and after prediction trims to see why the space is pruned ahead of the overlap.
3. Consider deferring prediction registration until the first authoritative cursor frame arrives (or until we have a stable prompt width) for the initial prompt line.
4. Explore holding spaces/newlines in a staging buffer while `cursor_authoritative_pending` is true, replaying them once the server echoes the prompt.

For any follow-up work, keep using the litmus script above and `scripts/analyze-predictive-trace.py` to compare traces before and after changes.

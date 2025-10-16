# Predictive Echo Regression — Diagnosis & Fix Plan

## Context

- Client build: `beach` (Rust TUI) — branch HEAD after predictive trace logging landed.
- Environment: macOS local client + remote host `ec2-user@54.169.75.185` (Singapore).  Session started via
  ```bash
  rm -f /tmp/beach-debug.log \
    && BEACH_LOG_FILTER=debug,client::predictive=trace cargo run -p beach -- \
         --log-level debug \
         --log-file /tmp/beach-debug.log \
         ssh --ssh-flag=-i --ssh-flag=/Users/arellidow/.ssh/beach-test-singapore.pem ec2-user@54.169.75.185
  ```
- Predictive trace analysis run with
  ```bash
  python scripts/analyze-predictive-trace.py /tmp/beach-debug.log --verbose
  ```
- Analyzer output contained 1,091 predictive events for source `rust_cli` with multiple mismatches and uncleared overlays.

## Observed Failures

The forensic script highlighted two dominant failure modes:

1. **Server content mismatches** — e.g.
   - `seq 5` mismatch at `(row=0, col=32)`
   - `seq 7` mismatch at `(row=0, col=35)`
   - `seq 13` mismatch at `(row=2, col=33)`
   - `seq 32` mismatch at `(row=2, col=50)`
   - Similar mismatches continue through seq ≥ 60.

   These correspond with the visual symptom: predictive characters persist even after the authoritative echo arrives, producing doubled characters and misaligned cursor position.

2. **ACK without server overlap** — many sequences (`seq 10/11/12/16/20/21/25…`) show:
   - `had no server overlap events`
   - `acked but predictions never cleared`

   In these cases the ACK arrives but no authoritative frame touches the predicted cells that were emitted, leaving the overlay in place until a later sweep (e.g. overlay pruning or a subsequent prediction) clears it.

Additional anomalies:

- `seq 8` and `seq 61` were reported as “acknowledged/cleared without registration,” implying we received ACKs for sequence IDs that the client never registered (likely out-of-order clearing after the prediction map was already truncated).
- Registered predictions often clear with reason `overlay_absent`, suggesting the renderer already trimmed the predictions before ACK processing, but the client state still believes they exist (stale positions).

## Likely Root Causes

Based on the traces and existing predictive logic:

1. **Cursor row drift vs. prediction row**
   - Mismatches predominantly occur on row 0 and row 2 (prompt & next line).  The analyzer shows the authoritative updates touching different columns than predicted.  Our predictive `register_prediction` uses `self.cursor_row`/`self.cursor_col`, which can be stale when the authoritative cursor (from server) lags.  That means predicted characters land on row 2 while the server writes them on row 1 or vice versa, so when the authoritative frame arrives, the row/col combination doesn’t match.

2. **Pending prediction map not pruned on trims/deltas**
   - When the renderer internally clears predictions (e.g. due to scrollback trims or overlay update), the `PendingPrediction` state still records positions, so the analyzer notes `overlay_absent` clears.  This indicates that renderer state and `pending_predictions` drift apart.  When the ACK arrives, the renderer no longer has the predictions, but the pending map does, so we get the confusing `cleared` reason.

3. **ACK handling assumes authoritative frames will touch positions**
   - For sequences where the server echoed back but the grid was already correct (e.g. because a later delta wrote the same text), the analyzer reports “no server overlap events” because the authoritative frame that matched the predicted characters may have arrived before the prediction was registered or after the pending positions were purged.  This points at race ordering between prediction registration and delta application.

4. **Sequence wrap/duplication**
   - `seq 8` & `seq 61` acked without registration implies prediction state was reset (buffer overflow or row trim) while the server still used the old sequence number.  When we hit the pending-prediction cap (256) we call `self.reset_prediction_state()`; any ACKs in flight for older sequences will hit the “missing” path.

## Fix Plan

### 1. Align predictive cursor with authoritative cursor updates
- **Action**: After processing each server delta/snapshot, reconcile the predictive cursor with `renderer.predicted_row_width`.  Ensure `register_prediction` pulls cursor row/col *after* ingesting authoritative cursor frames for the same watermark.
- **Change**: Delay applying predictions in `register_prediction` until after we’ve checked `self.cursor_authoritative_pending`.  If the server indicates the cursor is authoritative, flush predictions before writing new ones.

### 2. Synchronize renderer & pending prediction lifecycle
- **Action**: Whenever the renderer clears predictions (e.g. `clear_prediction_seq`, `clear_all_predictions`, `shrink_row_to_column`), mirror the change in `pending_predictions`.  Add helper functions to remove positions for a given `(row, col)` to keep both structures in sync.
- **Action**: On overlay pruning (`update_prediction_overlay`), when we see `has_predictions=false`, purge all pending records.

### 3. Improve ACK handling sequence
- **Action**: Keep the sent timestamp and row/column snapshot on the `PendingPrediction` struct and verify they still exist before removing.  If a prediction disappears before its ACK, record a `dropped_before_ack` reason instead of re-inserting.
- **Action**: When an ACK arrives for an unregistered seq, log a warning with the outstanding sequence range to help catch wrap vs. reset.

### 4. Adjust registration heuristics
- **Action**: Skip prediction registration when cursor support is authoritative and we have not yet processed the authoritative cursor for that watermark.  Instead, queue the prediction bytes and replay once the cursor is synced (or discard if the server already echoed them).
- **Action**: Clamp predictions to a single row when the server is still in prompt line (common case where row drift occurs).  If we see newline predictions while cursor_authoritative_pending is true, hold them until the authoritative cursor arrives.

### 5. Add regression tests
- **Rust TUI**:
  - Extend existing tests (e.g. `predictive_space_ack_clears_overlay`) to cover ack-without-overlap and mismatched row scenarios using injected deltas.
  - Add a test for prediction queue overflow (more than 256 pending) to ensure new logic handles in-flight ACKs gracefully.
- **Web client** (optional follow-up):  Mirror the analyzer’s failure cases by replaying the recorded sequences and assert grid state matches after each ACK.

### 6. Telemetry and tooling adjustments
- **Action**: Keep `client::predictive` logs at `debug` level so they’re available without changing the compile-time log level ceiling.
- **Action**: Add a short doc snippet describing how to collect predictive traces (`docs/predictive_logging.md`) and link to the analyzer script.

## Next Steps

1. Implement predictive state synchronization changes (items 1–4).
2. Add/extend unit tests to catch the regression.
3. Re-run the SSH session + analyzer to confirm mismatches disappear (expect no “mismatch” and no “acked but never cleared” rows).
4. Document the predictive trace workflow (include command + analyzer usage).
5. Consider merging the analyzer into CI (optional) for future regressions.

Once these steps are complete we should have both the predictive overlay and the analyzer reporting clean sequences again, matching what users expect from predictive echo.

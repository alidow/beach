# Predictive Echo Cursor Position Bug - Root Cause Hypothesis

**Date**: 2025-10-06
**Status**: Hypothesis pending validation

## Executive Summary

The predictive echo cursor drift is caused by Beach treating server cursor position updates as "lagging behind" predictions, rather than as authoritative corrections. When predictions don't match server state, Beach adjusts the server position forward to preserve predictions instead of discarding the incorrect predictions.

## Observed Symptoms

- Predictive overlay drifts right during fast typing, especially after spaces
- Analyzer reports consistent mismatches: `seq N row=R col=C mismatch`
- Pattern shows: predicted character at column C doesn't match server's character at column C
- Drift compounds over time (off by 1, then 2, then 3, etc.)
- Most visible on:
  - Row 0, cols 31-32 (initial prompt line)
  - Row 2, cols 30-38 (sustained drift after newline)

## Root Cause Analysis

### The Fatal Flaw in `apply_wire_cursor()` (terminal.rs:1697-1737)

When the server sends an authoritative cursor position update, Beach does this:

```rust
// Line 1725-1728
let adjusted_predicted = self.renderer.predicted_row_width(new_row as u64);
if adjusted_predicted > target_col {
    target_col = adjusted_predicted;  // ← THE BUG: Trust prediction over server
}
```

**What this means:**
- Server says: "cursor is at column 31"
- Client has predictions extending to column 32
- Code decides: "Server must be behind due to latency, move cursor to 32"
- **This is wrong**: The predictions were placed at the wrong position to begin with

### How It Cascades

1. User types 'o', client predicts it at column 31
2. Client updates internal state: `self.cursor_row = 0; self.cursor_col = 32`
3. Server responds: "cursor is at column 31" (server's prompt is shorter)
4. Bug activates: Code sees predictions at 32, adjusts server position from 31 → 32
5. User types ' ' (space), now predicting at column 32 when should be at 31
6. Off by one established
7. Next character off by 2, then 3, then 4...

### The Compound Problem

Beach has TWO places where predictions corrupt cursor state:

**Problem 1: Predictions mutate internal cursor state** (register_prediction:3475-3476)
```rust
if cursor_changed {
    self.cursor_row = cursor_row;  // ← Prediction becomes internal state
    self.cursor_col = cursor_col;
}
```

**Problem 2: Server updates adjusted to match predictions** (apply_wire_cursor:1725-1728)
```rust
if adjusted_predicted > target_col {
    target_col = adjusted_predicted;  // ← Server position adjusted forward
}
```

Once `self.cursor_row` and `self.cursor_col` are corrupted by a wrong prediction, there's no clean server state to fall back to. The next prediction uses the wrong starting position, and the cycle continues.

## Comparison with Mosh (Working Implementation)

Mosh also predicts cursor position, but handles mismatches differently:

### Mosh's Approach

**1. Predictions are separate from server state**
```cpp
// init_cursor() - First prediction starts from SERVER position
cursors.push_back(ConditionalCursorMove(
    local_frame_sent + 1,
    fb.ds.get_cursor_row(),     // ← Server framebuffer cursor
    fb.ds.get_cursor_col(),
    prediction_epoch));
```

**2. Subsequent predictions build on predicted cursor**
```cpp
// init_cursor() - Chain predictions together
cursors.push_back(ConditionalCursorMove(
    local_frame_sent + 1,
    cursor().row,    // ← Previous prediction
    cursor().col,
    prediction_epoch));
```

**3. When wrong, discard ALL predictions and restart from server**
```cpp
// cull() - Validation against server framebuffer
if (cursor().get_validity(fb, ...) == IncorrectOrExpired) {
    reset();  // Clear cursors list entirely
    return;
}
```

**4. Server framebuffer cursor is NEVER adjusted for predictions**
- The `Framebuffer` object maintains authoritative server cursor position
- Predictions live in separate `PredictionEngine` overlay
- Display renders: `server_framebuffer + prediction_overlays`
- Server updates NEVER look at predictions

### Key Architectural Difference

**Mosh decision flow:**
```
Server cursor != Predicted cursor
→ Predictions were wrong
→ Discard all predictions
→ Reset to server truth
```

**Beach decision flow (BROKEN):**
```
Server cursor != Predicted cursor
→ Server must be behind (network latency)
→ Adjust server cursor forward to match predictions
→ Preserve predictions
→ Continue with corrupted state
```

## Evidence from Logs

From `/tmp/beach-debug.log` analyzer output:

```
seq 4
  registered at 2798.939 ms
  cursor_before: {"col":31,"row":0,"seq":27}
  overlap at 2825.073 ms -> seq=4 row=0 col=31 mismatch
  predicted: 'o', server: 'c'

seq 5
  registered at 2863.880 ms
  cursor_before: {"col":32,"row":0,"seq":29}  ← Already off by 1
  overlap at 2953.642 ms -> seq=5 row=0 col=32 mismatch
  predicted: ' ', server: 'h'
```

**What happened:**
1. Seq 4: Predicted 'o' at col 31, server had something else → client cursor moved to 32
2. Seq 5: Started prediction from corrupted cursor position 32 instead of 31
3. Every subsequent character inherits the offset and adds to it

**Row 2 pattern (cols 30-38):**
- seq 10: mismatch at col 30 (predicted 'c', server 'e')
- seq 13: mismatch at col 31 (predicted ' ', server 'c')
- seq 14: mismatch at col 33 (predicted 'w', server 'o')
- seq 16: mismatch at col 35 (predicted 'r', server 'w')
- seq 17: mismatch at col 36 (predicted 'l', server 'o')
- seq 19: mismatch at col 38 (predicted 'k', server 'l')

Each mismatch shows the client is consistently ahead of where the server actually put the characters.

## Why Current Fixes Haven't Worked

Previous session added:
- Dropped-prediction tracking
- Synced renderer and pending prediction lifecycles
- Improved ACK handling with drop dwell time
- Cursor advancement logic changes
- Rebase predictions for prompt rewrites
- Regression tests

**These all work around the symptom but don't fix the root cause:** The core logic still trusts predictions over server cursor position.

## Proposed Fix Strategy

### Option 1: Discard predictions on mismatch (Mosh approach)

When server cursor position doesn't match expected position:
1. Clear ALL predictions
2. Reset internal cursor to server's authoritative position
3. Don't adjust server position to preserve predictions

**Change required in `apply_wire_cursor()`:**
```rust
// REMOVE lines 1725-1728:
// let adjusted_predicted = self.renderer.predicted_row_width(new_row as u64);
// if adjusted_predicted > target_col {
//     target_col = adjusted_predicted;
// }

// Instead: discard predictions that extend past server cursor
if predicted_width > target_col {
    self.discard_predictions_from_column(new_row, target_col);
}

// Always trust server position
self.cursor_row = new_row;
self.cursor_col = target_col;  // NOT adjusted_predicted
```

### Option 2: Separate prediction cursor from internal cursor state

Maintain two cursor positions:
- `self.cursor_row/col` = authoritative server position (never touched by predictions)
- `self.predicted_cursor_row/col` = predicted position (for display only)

Predictions use `predicted_cursor_*`, server updates only modify `cursor_*`.

**This is more invasive but architecturally cleaner.**

### Option 3: Defer prediction registration until stable cursor

For initial prompt lines, don't register predictions until the first authoritative cursor frame arrives. This prevents predicting during prompt rewrite sequences.

**This is a workaround, not a root fix.**

## Testing Strategy

### Before/After Litmus Test

```bash
rm -f /tmp/beach-debug.log && \
  BEACH_LOG_FILTER=debug,client::predictive=trace cargo run -p beach -- \
    --log-level trace \
    --log-file /tmp/beach-debug.log \
    ssh --ssh-flag=-i --ssh-flag=/Users/arellidow/.ssh/beach-test-singapore.pem \
        ec2-user@54.169.75.185
```

Type quickly: `echo "hello world"` + space

**Expected after fix:**
- Analyzer shows NO mismatches at cursor positions
- NO "acked but predictions never cleared" errors
- Predictions match server content or are properly discarded

### Unit Test Cases

1. **Server cursor behind predicted cursor** → predictions discarded
2. **Server cursor at predicted position** → predictions confirmed
3. **Rapid typing with prompt rewrite** → cursor resyncs correctly
4. **Newline prediction with cursor row change** → no drift after line break

## Next Steps

1. Implement Option 1 (simplest, proven by Mosh)
2. Run litmus test and analyzer
3. If mismatches persist, implement Option 2 (architectural separation)
4. Add regression tests for cursor position trust
5. Consider Option 3 as additional defense layer

## Open Questions

1. Are there legitimate cases where server cursor IS behind due to backfill/replay?
2. Should we ever trust predictions over server state?
3. How does cursor_authoritative_pending flag interact with this logic?
4. Do we need epoch/sequence tracking like Mosh to handle out-of-order updates?

## References

- Handoff notes: `docs/predictive-echo-handoff.md`
- Mosh source: `terminaloverlay.cc` (PredictionEngine::cull, init_cursor, apply)
- Beach code: `apps/beach/src/client/terminal.rs` (apply_wire_cursor:1697-1737, register_prediction:3353-3544)
- Analyzer script: `scripts/analyze-predictive-trace.py`

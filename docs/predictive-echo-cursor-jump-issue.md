# Predictive Echo Cursor Jump Issue

**Status**: UNRESOLVED
**Date**: 2025-10-07
**Severity**: High - Affects user experience significantly

## Problem Statement

Beach's predictive echo implementation causes visible cursor jumping and artifacts during fast typing, particularly after space characters. The cursor appears to "jump back and rewrite" content that was already predicted, creating a jarring visual experience. This behavior does NOT occur in Mosh, which has smooth, artifact-free predictive echo.

### Observable Symptoms

1. **Cursor jumps backwards during typing**
   - Type "asdf" quickly → all characters appear underlined, then cursor jumps back and rewrites "asdf"
   - This creates a visible "flash" or "stutter" effect

2. **Space bar causes positioning issues**
   - Type "echo test" → after space, the next character sometimes appears at the wrong position
   - Character appears where the space is, then gets cleared when server confirms

3. **Out-of-order prediction clearing** (PARTIALLY FIXED)
   - Older characters show underlines while newer characters don't
   - Predictions clear based on ACK timing rather than sequence order

4. **Cursor position inconsistency**
   - Display cursor doesn't match where next character will be typed
   - Server cursor updates cause visible jumps even when predictions are active

## Expected Behavior (Mosh)

In Mosh:
- Cursor NEVER jumps backwards
- Predicted characters appear instantly with underline styling
- As server confirms, underlines disappear smoothly from left to right
- No visible rewrites or position jumps
- Predictions are purely visual overlays that don't affect terminal state

## Technical Context

### Architecture

Beach's predictive echo system has these components:

1. **Terminal State** (`apps/beach/src/client/terminal.rs`)
   - `cursor_row`, `cursor_col` - Display cursor position
   - `server_cursor_row`, `server_cursor_col` - Authoritative server position
   - `pending_predictions` - HashMap of predictions awaiting confirmation
   - `dropped_predictions` - Predictions that have been cleared

2. **Grid Renderer** (`apps/beach/src/client/grid_renderer.rs`)
   - `predictions` - HashMap of predicted cells `(row, col) -> PredictedCell`
   - Renders predictions as overlays with underline styling
   - `cell_matches()` - Checks if predicted character matches actual terminal content

3. **Prediction Lifecycle**
   ```
   User Input → register_prediction() → add to pending_predictions
                                      → renderer.add_prediction()
                                      → update display cursor

   Server ACK → ack_prediction() → mark prediction.acked_at

   Render Loop → prune_acked_predictions() → try_clear_prediction()
                                           → check if committed or grace period expired
                                           → renderer.clear_prediction_seq()
   ```

4. **Key Functions**
   - `register_prediction(seq, bytes)` - Creates new prediction (line 3439)
   - `ack_prediction(seq)` - Marks prediction as acknowledged by server (line 3332)
   - `prune_acked_predictions()` - Clears expired predictions (line 3648)
   - `try_clear_prediction()` - Attempts to clear a single prediction (line 3679)
   - `apply_wire_cursor()` - Updates cursor from server frame (line 1755)
   - `update_cursor_from_predictions()` - Updates display cursor to predicted position (line 4005)
   - `latest_prediction_cursor()` - Finds cursor position of latest prediction (line 3992)

### Core Issue: Cursor State Management

The fundamental problem is mixing display cursor state with prediction base cursor:

```rust
// In register_prediction() - line 3483
let (cursor_row, cursor_col) = if let Some((_, row, col)) = self.latest_prediction_cursor() {
    (row, col)  // Use latest prediction's end position
} else {
    (self.server_cursor_row, self.server_cursor_col)  // Use server position
};

// In apply_wire_cursor() - line 1810
if !self.predictive_input || !self.renderer.has_active_predictions() {
    self.cursor_row = new_row;  // Update display cursor from server
    self.cursor_col = target_col;
    self.sync_renderer_cursor();
}
```

When server frames arrive while predictions are active:
1. Server sends cursor position for each character echo
2. Each frame tries to update display cursor
3. Even with the guard at line 1810, cursor state gets inconsistent
4. Result: visible jumps as cursor position changes

### Attempted Fixes (All Failed)

#### Attempt 1: Remove cursor position override (Bug #1 & #2)
- **Issue**: Server cursor was being adjusted forward to match predictions
- **Fix**: Always trust server cursor, never adjust (lines 1794-1797)
- **Result**: Fixed drift, but cursor still jumps

#### Attempt 2: Track max predicted cursor position
- **Issue**: Cursor stepped backwards as predictions cleared one-by-one
- **Fix**: Added `max_predicted_cursor_row/col` fields to track furthest position
- **Result**: Caused worse space bar issues, reverted

#### Attempt 3: Don't update cursor when dropping predictions
- **Issue**: Clearing predictions called `refresh_prediction_cursor()` causing backwards steps
- **Fix**: Changed to `sync_renderer_cursor()` (lines 3818, 3834)
- **Result**: Reduced backwards jumping, but still present

#### Attempt 4: Enforce sequence order for clearing
- **Issue**: Predictions cleared out of order based on ACK timing
- **Fix**: Sort by sequence, only clear if no earlier sequences pending (lines 3666-3676)
- **Result**: Fixed out-of-order underlines, but cursor still jumps

#### Attempt 5: Build predictions from latest_prediction_cursor()
- **Issue**: New predictions used display cursor which could be stale
- **Fix**: Use `latest_prediction_cursor()` as base (lines 3481-3489)
- **Result**: Improved chaining, but cursor still jumps after server updates

#### Attempt 6: Don't update display cursor when predictions active
- **Issue**: Server frames updated display cursor causing jumps
- **Fix**: Guard `cursor_row/col` update if predictions active (lines 1807-1814)
- **Result**: Still jumps - guard isn't sufficient

## Testing Methodology

### Test Setup

1. **High-latency SSH connection** (Singapore server, ~250ms RTT)
   ```bash
   cargo run -p beach-human -- ssh --ssh-flag=-i --ssh-flag=/Users/arellidow/.ssh/beach-test-singapore.pem ec2-user@54.169.75.185
   ```

2. **Local test with artificial latency**
   ```bash
   # Terminal 1: Host
   cargo run -p beach-human -- host --local-preview

   # Terminal 2: Client with latency injection
   cargo run -p beach-human -- join <SESSION_ID> --passcode <PASSCODE> --inject-latency 250
   ```

3. **Automated test with trace logging**
   ```bash
   BEACH_LOG_FILTER=debug,client::predictive=trace \
   cargo run -p beach-human -- \
     --log-level trace \
     --log-file /tmp/beach-test.log \
     ssh <args>
   ```

### Test Scripts

Located in `/tmp/`:
- `test-predictive-echo-automated.sh` - Full automated test with AppleScript
- `launch-beach-client-fixed.sh` - Launches host + client with latency injection
- `send-test-input.sh` - Sends test input via IPC to running session
- `test-predictive-bug-interactive.sh` - Semi-automated test with analysis

### Mosh Comparison Test

```bash
# Run Mosh to Singapore server
mosh ec2-user@54.169.75.185

# Type quickly: "asdf", "echo test", "ls -la /etc"
# Observe: Cursor NEVER jumps backwards, no visual rewrites
```

### Log Analysis

```bash
# Count prediction events
grep -c '"event":"prediction_registered"' /tmp/beach-test.log
grep -c '"event":"prediction_ack"' /tmp/beach-test.log
grep -c '"event":"prediction_cleared"' /tmp/beach-test.log
grep -c '"event":"prediction_clear_deferred"' /tmp/beach-test.log

# Analyze with Python script
python scripts/analyze-predictive-trace.py /tmp/beach-test.log --verbose
```

Expected healthy metrics:
- Registered ≈ Acked ≈ Cleared (all predictions processed)
- Clear deferred count < 100 (minimal deferrals)
- Predictions clear via "ack_expired" or "committed" reasons

## Key Findings

### What Works
1. ✅ Predictions are registered correctly
2. ✅ Server ACKs predictions reliably
3. ✅ Predictions clear eventually (84%+ clear rate)
4. ✅ Sequence-order clearing prevents out-of-order underlines
5. ✅ Predictions chain together via `latest_prediction_cursor()`

### What Doesn't Work
1. ❌ Display cursor jumps backwards during server frame processing
2. ❌ Visual "rewrite" effect as cursor moves back and forward
3. ❌ Space bar causes next character to appear at wrong position sometimes
4. ❌ Guard at line 1810 isn't preventing all cursor updates

### Mosh vs Beach Architecture Difference

**Mosh's approach (from research/documentation):**
- Predictions are PURELY visual overlays
- Display cursor is calculated: `base_cursor + predicted_offset`
- Server cursor NEVER directly updates display cursor
- Predictions don't mutate any terminal state
- Display updates are atomic: either show predictions OR show server state, never mixed

**Beach's current approach:**
- Predictions mutate display cursor (`self.cursor_row/col`)
- Server frames try to update same cursor state
- Display cursor is shared between prediction system and server updates
- Creates race condition where both systems fight for cursor control

## Root Cause Hypothesis

The core issue is **shared mutable cursor state**. Both the prediction system and server frame processing update `self.cursor_row/col`, creating a race condition:

```rust
// Prediction system updates cursor (line 3574-3577)
if let Some((_, row, col)) = self.latest_prediction_cursor() {
    self.cursor_row = row;
    self.cursor_col = col;
}

// Server frame processing ALSO updates cursor (line 1811-1812)
if !self.predictive_input || !self.renderer.has_active_predictions() {
    self.cursor_row = new_row;  // CONFLICT!
    self.cursor_col = target_col;
}
```

Even with the guard, there are edge cases where:
1. Predictions exist but `has_active_predictions()` returns false (race)
2. Server frames arrive between prediction registration and cursor update
3. Multiple frames process in quick succession, each updating cursor

## Recommended Solution

Separate display cursor from server cursor completely:

1. **Never mutate `cursor_row/col` from server frames when predictions exist**
2. **Calculate display cursor position only during rendering**
3. **Use separate fields:**
   - `server_cursor_row/col` - Authoritative position from server (never shown during predictions)
   - `display_cursor_row/col` - What user sees (computed from predictions)
   - `prediction_base_cursor_row/col` - Starting point for new predictions

4. **Rendering logic:**
   ```rust
   fn compute_display_cursor(&self) -> (usize, usize) {
       if self.predictive_input && !self.pending_predictions.is_empty() {
           if let Some((_, row, col)) = self.latest_prediction_cursor() {
               return (row, col);  // Show predicted position
           }
       }
       (self.server_cursor_row, self.server_cursor_col)  // Show server position
   }
   ```

5. **Frame processing:**
   ```rust
   fn apply_wire_cursor(&mut self, frame: &WireTerminalCursor) {
       // ONLY update server cursor, NEVER display cursor
       self.server_cursor_row = new_row;
       self.server_cursor_col = target_col;
       // Display cursor is computed during render, not here
   }
   ```

This matches Mosh's architecture: predictions are visual overlays, server state is authoritative, display is computed not mutated.

## Alternative Approaches

### Approach A: Disable cursor updates during prediction window
- Suppress ALL cursor updates from server while predictions pending
- Risk: Cursor could get stuck if predictions don't clear properly

### Approach B: Buffer server cursor updates
- Queue server cursor updates while predictions active
- Apply buffered updates only when predictions clear
- Risk: Complex state management, potential memory growth

### Approach C: Atomic display updates
- Render either "prediction view" (predicted cursor) OR "server view" (server cursor)
- Never mix the two within a single frame
- Requires more invasive rendering changes

## Files Modified (Current State)

All changes in `apps/beach/src/client/terminal.rs`:

1. **Line 1794-1814**: `apply_wire_cursor()` - Guard display cursor update if predictions active
2. **Lines 3481-3489**: `register_prediction()` - Use `latest_prediction_cursor()` as base
3. **Lines 3571-3578**: `register_prediction()` - Update display cursor after registration
4. **Lines 3666-3676**: `prune_acked_predictions()` - Enforce sequence order for clearing
5. **Lines 3818, 3834**: Prediction drop functions - Use `sync_renderer_cursor()` instead of `refresh_prediction_cursor()`
6. **Lines 4005-4017**: `update_cursor_from_predictions()` - Simplified without max tracking

## Related Files

- `apps/beach/src/client/grid_renderer.rs` - Prediction overlay rendering
- `apps/beach/src/client/terminal/join.rs` - Join command with `--inject-latency` flag
- `apps/beach/src/client/terminal/debug.rs` - Debug IPC commands for testing
- `apps/beach/src/terminal/cli.rs` - CLI argument definitions
- `docs/predictive-echo-testing-guide.md` - Testing procedures
- `docs/predictive-echo-handoff.md` - Original bug report
- `docs/predictive-echo-cursor-bug-hypothesis.md` - Initial hypothesis
- `scripts/analyze-predictive-trace.py` - Log analysis tool

## Next Steps for Future Work

1. **Study Mosh source code** in detail:
   - How does Mosh separate display cursor from state cursor?
   - How does Mosh handle cursor updates during prediction window?
   - What's Mosh's exact algorithm for computing display cursor?

2. **Refactor cursor state management**:
   - Create separate `DisplayCursor` and `ServerCursor` types
   - Enforce separation at the type level
   - Compute display cursor only during rendering, never mutate directly

3. **Add comprehensive cursor tracing**:
   - Log every cursor update with source (prediction/server/render)
   - Add frame-by-frame cursor position timeline to logs
   - Capture exact timing of cursor jumps

4. **Create minimal reproduction**:
   - Simplify test case to single prediction + single server update
   - Use IPC to inject exact timing of input and server frames
   - Isolate the exact moment cursor jumps occur

5. **Consider renderer-level cursor handling**:
   - Move cursor position calculation entirely into GridRenderer
   - GridRenderer computes display cursor based on predictions
   - Terminal never mutates cursor during frame processing

## References

- Mosh source: https://github.com/mobile-shell/mosh
- Mosh prediction logic: `src/frontend/terminaloverlay.cc`
- Beach predictive echo: `apps/beach/src/client/terminal.rs:3439-3850`

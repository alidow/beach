# Predictive Echo Testing Guide

This document describes how to test and debug the predictive echo functionality in Beach terminal sessions, including automated testing procedures and diagnosis workflows.

## Overview

Predictive echo allows the client to display local predictions of user input before receiving server confirmation. This improves perceived responsiveness, especially on high-latency connections. However, bugs in the prediction clearing logic can cause cursor drift and visual artifacts.

## Bug Symptoms

- Cursor position drifts to the right when typing quickly
- Predicted characters don't match final server output
- Predictions accumulate but never clear
- Visible misalignment between predicted and actual terminal state

## Testing Infrastructure

### Components

1. **Latency Injection** (`--inject-latency` flag)
   - Artificially delays server frame processing
   - Simulates high-latency connections (e.g., 250ms)
   - Enables local reproduction of latency-dependent bugs

2. **IPC Input Sending** (`debug --send` command)
   - Sends input to a running interactive client via IPC
   - Bypasses need for manual typing in tests
   - Enables automated test scenarios

3. **Predictive Trace Logging** (`BEACH_LOG_FILTER=debug,client::predictive=trace`)
   - Emits detailed JSON events for prediction lifecycle
   - Events: `prediction_registered`, `prediction_ack`, `prediction_cleared`, `prediction_clear_deferred`
   - Analyzable with `scripts/analyze-predictive-trace.py`

### Automated Test Scripts

Located in `/tmp/`:

- **`launch-beach-client-fixed.sh`** - Full automation: starts host + AppleScript-launched client
- **`send-test-input.sh <session-id>`** - Sends "echo hello" to session via IPC
- **`test-predictive-bug-interactive.sh <session-id>`** - Semi-automated test with analysis

## Running Automated Tests

### Full Automation

```bash
# Clean up any existing sessions
pkill -f "beach-human" 2>/dev/null || true

# Run automated test
/tmp/launch-beach-client-fixed.sh

# The script will:
# 1. Start beach host with --local-preview
# 2. Extract session ID and passcode
# 3. Launch client in Terminal.app via AppleScript with 250ms latency injection
# 4. Display session info for IPC commands

# In another terminal, send test input
SESSION_ID="<from-launch-script-output>"
/tmp/send-test-input.sh "$SESSION_ID"

# Wait for processing
sleep 3

# Analyze results
python scripts/analyze-predictive-trace.py /tmp/beach-interactive-client.log --verbose

# Check event counts
grep -c '"event":"prediction_registered"' /tmp/beach-interactive-client.log
grep -c '"event":"prediction_ack"' /tmp/beach-interactive-client.log
grep -c '"event":"prediction_cleared"' /tmp/beach-interactive-client.log
grep -c '"event":"prediction_clear_deferred"' /tmp/beach-interactive-client.log

# Clean up
pkill -f "beach-human"
```

### Manual Interactive Testing

```bash
# Terminal 1: Start host
cargo run -p beach-human -- host --local-preview

# Terminal 2: Start client with trace logging and latency injection
BEACH_LOG_FILTER=debug,client::predictive=trace \
cargo run -p beach-human -- \
  --log-level trace \
  --log-file /tmp/beach-test.log \
  join <SESSION_ID> --passcode <PASSCODE> \
  --inject-latency 250

# Terminal 3: Send automated input
/tmp/test-predictive-bug-interactive.sh <SESSION_ID> /tmp/beach-test.log

# Clean up
pkill -f "beach-human"
```

## Interpreting Results

### Healthy Prediction Lifecycle

```
1. prediction_registered  - User input creates prediction
2. prediction_ack        - Server ACKs the input sequence
3. prediction_cleared    - Prediction removed after server confirms
```

### Bug Pattern (Current State)

```
1. prediction_registered       - ✓ Prediction created
2. prediction_ack             - ✓ Server ACKs input
3. prediction_clear_deferred  - ⚠️  Clearing deferred (repeats thousands of times)
4. prediction_cleared         - ✗ NEVER occurs (BUG!)
```

### Key Metrics

- **Predictions registered**: Should match number of input characters
- **Predictions ACKed**: Should match registered count
- **Predictions cleared**: Should match ACKed count (currently 0 = BUG)
- **Clear deferred events**: Should be minimal (currently thousands = BUG)

### Analyzer Output

```bash
python scripts/analyze-predictive-trace.py /tmp/beach-test.log --verbose
```

Look for:
- `had no server overlap events` - No comparison between predicted/actual content
- `acked but predictions never cleared` - Core bug: predictions accumulate
- `server content did not match predictions` - Visible mismatch (indicates cursor drift)

## Known Issues

### Issue 1: Predictions Never Clear

**Symptom**: All predictions show "acked but predictions never cleared"

**Evidence**:
- `prediction_cleared` event count: 0
- `prediction_clear_deferred` event count: 4000-50000+
- Predictions accumulate indefinitely

**Root Cause Hypothesis**:
The `register_prediction` function mutates the internal cursor position (terminal.rs:3475-3476) without restoration when predictions are dropped. When predictions are ACKed and should be cleared, the clearing is deferred indefinitely because the cursor state is inconsistent.

**Location**: `apps/beach/src/client/terminal.rs:3475-3476`

```rust
// BUG: This mutates cursor without restoration
self.term.lock().grid_mut().cursor.point.column = Column(effective_col as usize);
```

### Issue 2: No Server Overlap Events

**Symptom**: Analyzer reports "had no server overlap events"

**Possible Causes**:
1. Simple test input ("echo hello") exactly matches server response
2. Overlap detection not triggering
3. Need more complex test scenarios (e.g., prompt rewrites after spaces)

## Diagnosis Plan

### Phase 1: Confirm Bug Replication ✅

- [x] Implement latency injection (`--inject-latency`)
- [x] Implement IPC input sending (`debug --send`)
- [x] Create automated test harness
- [x] Verify predictions are registered and ACKed
- [x] Confirm predictions never clear (0 `prediction_cleared` events)

### Phase 2: Identify Root Cause

- [ ] Add trace logging for cursor mutation in `register_prediction`
- [ ] Verify cursor_before vs cursor_after in logs
- [ ] Check if cursor restoration logic exists
- [ ] Compare with Mosh implementation (discards all predictions on mismatch)
- [ ] Identify why `prediction_clear_deferred` loops infinitely

### Phase 3: Implement Fix

**Option A: Restore Cursor After Prediction Drop**
- Save cursor position before prediction
- Restore cursor when predictions are cleared
- Ensure cursor restoration happens in all clear paths

**Option B: Don't Mutate Cursor (Mosh-style)**
- Use prediction overlay without mutating terminal state
- Keep predictions separate from authoritative cursor
- Only update cursor on server confirmation

**Option C: Hybrid Approach**
- Allow cursor mutation for predictions
- When server cursor doesn't match predictions, discard ALL predictions
- Don't try to adjust server position to match predictions

### Phase 4: Validate Fix

- [ ] Run automated test suite
- [ ] Verify `prediction_cleared` events occur
- [ ] Verify `prediction_clear_deferred` count is minimal
- [ ] Test with complex scenarios (prompt rewrites, multi-line, etc.)
- [ ] Test with real high-latency connections (Singapore SSH)
- [ ] Verify no cursor drift visible in Terminal.app

### Phase 5: Regression Testing

- [ ] Test without latency injection
- [ ] Test with varying latency levels (50ms, 100ms, 250ms, 500ms)
- [ ] Test rapid typing scenarios
- [ ] Test with different shells (zsh, bash, fish)
- [ ] Test with different prompt configurations

## Reference Files

### Implementation
- `apps/beach/src/client/terminal.rs` - Main terminal client, prediction logic
- `apps/beach/src/client/terminal/join.rs` - Join command, latency injection setup
- `apps/beach/src/client/terminal/debug.rs` - Debug IPC commands
- `apps/beach/src/terminal/cli.rs` - CLI argument definitions

### Analysis
- `scripts/analyze-predictive-trace.py` - Prediction trace analyzer
- `docs/predictive-echo-handoff.md` - Original bug handoff notes
- `docs/predictive-echo-cursor-bug-hypothesis.md` - Initial hypothesis

### Testing
- `/tmp/launch-beach-client-fixed.sh` - Full automated test
- `/tmp/send-test-input.sh` - IPC input sender
- `/tmp/test-predictive-bug-interactive.sh` - Semi-automated test

## Cleanup

Always clean up background processes after testing:

```bash
# Kill all beach processes
pkill -f "beach-human"

# Close Terminal.app tabs if using AppleScript automation
# (Manual cleanup required)

# Remove test logs (optional)
rm -f /tmp/beach-*.log
```

## Next Steps

1. Complete Phase 2: Root cause analysis with additional trace logging
2. Choose fix approach (A, B, or C based on findings)
3. Implement fix in `apps/beach/src/client/terminal.rs`
4. Run automated tests to validate
5. Test with real high-latency SSH connection to Singapore

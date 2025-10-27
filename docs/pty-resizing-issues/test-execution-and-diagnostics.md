# PTY Resize Test Execution and Diagnostics

This guide explains how to run the PTY resize duplication tests and collect diagnostic logs for debugging.

## Overview

We have automated tests that reproduce the PTY resize HUD duplication issue. This document covers:
- Running the standalone beach-surfer test
- Running the Private Beach tile resize test
- Collecting browser console logs
- Analyzing terminal trace data
- Interpreting Beach host logs

## Test Locations

### Standalone Test (beach-surfer)
**Location:** `apps/beach-surfer/tests/pty-resize-standalone.spec.ts`

**Purpose:** Tests PTY resize behavior in beach-surfer web client without Private Beach dependencies

**Advantages:**
- Simpler setup (no auth required)
- Faster iteration
- Direct terminal resize via viewport changes

### Private Beach Tile Test
**Location:** `apps/private-beach/tests/e2e/tile-resize-hud.spec.ts`

**Purpose:** Tests PTY resize behavior when resizing react-grid-layout tiles in Private Beach

**Advantages:**
- Tests real production integration
- Uses actual tile drag-to-resize mechanism
- Can achieve larger resizes (70+ rows)

## Prerequisites

### 1. Install Dependencies

```bash
# Install Playwright browsers (first time only)
cd apps/beach-surfer
npx playwright install

# Or for Private Beach tests
cd apps/private-beach
npx playwright install
```

### 2. Start Infrastructure

You need the following services running:

```bash
# Terminal 1: Redis
docker-compose up -d redis

# Terminal 2: beach-road (session server)
cargo run -p beach-road

# Terminal 3: Beach host with Pong demo
cargo run -p beach -- host --session-server http://localhost:8080 -- \
  python3 apps/private-beach/demo/pong/player/main.py --mode lhs

# Save the session credentials from output:
# - session_id: (from JSON bootstrap line)
# - join_code: (from JSON bootstrap line)
```

**Important:** Keep these terminals open during testing. The Beach host output contains valuable diagnostic information.

### 3. Start Web Client

**For beach-surfer test:**
```bash
# Terminal 4: beach-surfer dev server
cd apps/beach-surfer
npm run dev
```

**For Private Beach test:**
```bash
# Terminal 4: Private Beach dev server
cd apps/private-beach
npm run dev
```

## Running the Standalone Test

### Basic Execution

```bash
cd apps/beach-surfer

# Set environment variables with your session credentials
export BEACH_TEST_SESSION_ID="<session-id-from-beach-host>"
export BEACH_TEST_PASSCODE="<join-code-from-beach-host>"
export BEACH_TEST_SESSION_SERVER="http://localhost:8080"

# Run the test
npx playwright test pty-resize-standalone.spec.ts --workers=1
```

### With Visual Browser (Headed Mode)

```bash
npx playwright test pty-resize-standalone.spec.ts --headed --workers=1
```

### With Playwright UI (Interactive)

```bash
npx playwright test pty-resize-standalone.spec.ts --ui
```

This allows you to:
- Step through test actions
- Inspect DOM state at each step
- Manually interact with the page
- See browser console logs in real-time

### Expected Output

```
Initial state: { viewportRows: 35, gridRows: 24, rowsLoaded: 35 }
After resize: { viewportRows: 36, gridRows: 36, rowsLoaded: 36 }
Found 6 duplicate rows
Duplicates: [
  {
    row1: 0,
    row2: 2,
    text: '|  #                                    |                                      |'
  },
  ...
]
```

If duplicates are found, the test will fail and generate:
- Screenshot: `test-results/*/test-failed-1.png`
- Trace file: `test-results/*/trace.zip`
- Console logs: Shown in test output

## Collecting Diagnostic Logs

### 1. Browser Console Logs

The test automatically enables Beach trace logging. To see detailed console output:

```bash
# Run with verbose console output
DEBUG=pw:browser npx playwright test pty-resize-standalone.spec.ts --workers=1
```

**Console logs are automatically captured in:**
- Test stdout (shown during test run)
- Playwright trace (viewable in trace viewer)

### 2. Terminal Trace Data

The test uses `window.__BEACH_TRACE_DUMP_ROWS()` to capture terminal state. This data is logged to console:

```javascript
// Before resize
{
  viewportHeight: 35,
  rowCount: 24,
  baseRow: 0,
  followTail: true,
  rows: [
    { kind: 'live', absolute: 11, text: '|  #  ...', seq: 123 },
    ...
  ]
}

// After resize
{
  viewportHeight: 70,
  rowCount: 70,
  baseRow: 0,
  followTail: true,
  rows: [
    // All 70 rows with full content
  ]
}
```

**Key fields:**
- `viewportHeight`: How many rows the terminal viewport can display
- `rowCount`: Total rows in the grid (includes scrollback)
- `baseRow`: First visible row index
- `followTail`: Whether terminal is following latest output
- `rows[]`: Array of row objects with text content and metadata

### 3. Beach Host Logs

The Beach host terminal shows crucial information about PTY resize events:

```bash
# Look for PTY resize events
INFO beach::server::terminal::pty: pty resized rows=36 cols=80

# Look for backfill operations
INFO beach::sync::backfill: scheduling backfill start_seq=100 end_seq=200

# Look for transport activity
INFO transport established transport="webrtc" peer_id=...
INFO queueing outbound message payload_len=11667 sequence=6
```

**To save Beach host logs:**
```bash
# Run Beach host with log file output
BEACH_LOG_FILE=/tmp/beach-host.log \
BEACH_LOG_LEVEL=info \
cargo run -p beach -- host --session-server http://localhost:8080 -- \
  python3 apps/private-beach/demo/pong/player/main.py --mode lhs
```

### 4. beach-road Server Logs

Monitor the session server for connection issues:

```bash
# Run beach-road with debug logging
RUST_LOG=debug cargo run -p beach-road 2>&1 | tee /tmp/beach-road.log
```

Look for:
- Session registration events
- WebRTC signaling activity
- Connection errors
- WebSocket issues

### 5. Playwright Trace Viewer

Generate and view detailed traces:

```bash
# Run with trace enabled
npx playwright test pty-resize-standalone.spec.ts --trace on

# Open trace viewer
npx playwright show-trace test-results/*/trace.zip
```

The trace viewer shows:
- Timeline of all test actions
- Network requests
- Console logs
- Screenshots at each step
- DOM snapshots

## Analyzing Test Results

### Understanding Duplicate Detection

The test identifies duplicates by:

1. Filtering out known repetitive content (borders, separators)
2. Comparing all remaining text rows pairwise
3. Finding exact text matches at different row indices

**Example duplicate:**
```javascript
{
  row1: 0,      // First occurrence at row 0
  row2: 2,      // Duplicate at row 2
  text: '|  #  ...'  // The duplicated content
}
```

### Common Patterns

**Paddle segment duplication:**
```
Row 0:  |  #                    |           |
Row 1:  |  #                                |
Row 2:  |  #                    |           |  <- Duplicate of row 0
```

**HUD text duplication:**
```
Row 10: Ready. Commands: m <delta> | ...
Row 11: Mode: LHS — Paddle X=3 (positive delta moves up)
Row 12: >
Row 25: Ready. Commands: m <delta> | ...  <- Duplicate HUD
Row 26: Mode: LHS — Paddle X=3 (positive delta moves up)
Row 27: >
```

### Diagnostic Checklist

When a test fails with duplicates:

1. **Check viewport size achieved**
   - Initial vs. after resize
   - Did it reach target size (70+ rows)?

2. **Review terminal trace data**
   - Are rows marked as 'live' or 'backfill'?
   - What is the `baseRow` value?
   - Is `followTail` true?

3. **Examine Beach host logs**
   - Was PTY resize event logged?
   - Were backfill operations triggered?
   - Check sequence numbers for gaps

4. **Look at screenshot**
   - Visual confirmation of duplication
   - Check if content is offset/misaligned

5. **Check timing**
   - Did test wait long enough after resize?
   - Was backfill still in progress?

## Test Configuration

### Timeouts

```typescript
// In test file
test.setTimeout(60000); // 60 second timeout

// Wait after resize for PTY backfill
await page.waitForTimeout(5000); // 5 seconds
```

Increase if backfill takes longer for your scenario.

### Viewport Sizes

```typescript
// Target: ~70 rows × 90 cols
await page.setViewportSize({ width: 1400, height: 1800 });
```

Adjust based on:
- Terminal font size
- Desired row/column count
- Browser window chrome

### Ignore Patterns

```typescript
const ignorePatterns = [
  /^\|[\s|]*\|$/,  // Pong borders
  /^[\s]*$/,        // Empty lines
];
```

Add patterns for content that should not be considered duplicates.

## Troubleshooting

### Test Timeout

**Symptom:** Test times out waiting for connection

**Solutions:**
- Verify beach-road is running on port 8080
- Check Beach host is running and session is active
- Ensure correct session ID and passcode
- Check firewall/network settings

### No Duplicates Found (False Negative)

**Symptom:** Test passes but duplicates are visible in screenshot

**Solutions:**
- Check ignore patterns aren't filtering too aggressively
- Increase viewport size to trigger larger resize
- Increase wait time after resize
- Review terminal trace data for actual row content

### Cannot Connect to Session

**Symptom:** Test fails with "connection error" or similar

**Solutions:**
- Verify session credentials are correct
- Check Beach host didn't crash or exit
- Ensure WebRTC/WebSocket connectivity
- Review beach-road logs for errors

### Flaky Tests

**Symptom:** Test passes sometimes, fails other times

**Solutions:**
- Increase timeouts after resize
- Add explicit waits for connection state
- Disable test parallelization (`--workers=1`)
- Check for resource contention (CPU/memory)

## Advanced Diagnostics

### Manual Testing

To manually reproduce and inspect:

1. Open browser to http://localhost:5173
2. Enter session credentials
3. Connect and wait for terminal to load
4. Open browser DevTools (F12)
5. In console, run:
   ```javascript
   window.__BEACH_TRACE = true;
   window.__BEACH_TRACE_DUMP_ROWS();
   console.log(window.__BEACH_TRACE_LAST_ROWS);
   ```
6. Resize browser window significantly taller
7. Wait 5-10 seconds
8. Run trace dump again:
   ```javascript
   window.__BEACH_TRACE_DUMP_ROWS();
   console.log(window.__BEACH_TRACE_LAST_ROWS);
   ```
9. Compare row content for duplicates

### Comparing Before/After State

Save trace dumps to files for diffing:

```javascript
// In browser console before resize
const before = window.__BEACH_TRACE_LAST_ROWS;
copy(JSON.stringify(before, null, 2));
// Paste into /tmp/before.json

// After resize
const after = window.__BEACH_TRACE_LAST_ROWS;
copy(JSON.stringify(after, null, 2));
// Paste into /tmp/after.json

// Diff the files
diff /tmp/before.json /tmp/after.json
```

### Monitoring Backfill in Real-Time

Add trace logging to Beach code:

```rust
// In apps/beach/src/sync/backfill.rs (or relevant file)
tracing::info!(
    "backfill scheduled",
    start_row = start,
    end_row = end,
    viewport_height = self.viewport_height,
);
```

Then run with trace level:

```bash
BEACH_LOG_LEVEL=trace cargo run -p beach -- host ...
```

## Related Documentation

- [Private Beach Duplicate HUD Issue](./private-beach-duplicate-hud.md)
- [beach-surfer Test README](../../apps/beach-surfer/tests/README.md)
- [Private Beach Test README](../../apps/private-beach/tests/e2e/README.md)

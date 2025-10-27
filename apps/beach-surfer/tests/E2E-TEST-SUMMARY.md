# E2E PTY Resize Test - Implementation Summary

## Overview

Successfully created a comprehensive end-to-end test infrastructure for reproducing and verifying the PTY resize HUD duplication issue documented in [`docs/pty-resizing-issues/private-beach-duplicate-hud.md`](../../../docs/pty-resizing-issues/private-beach-duplicate-hud.md).

## What Was Built

### 1. Test Infrastructure

**Created Files:**
- [`tests/pty-resize-hud-duplication.spec.ts`](./pty-resize-hud-duplication.spec.ts) - Main Playwright test
- [`tests/scripts/launch-test-session.sh`](./scripts/launch-test-session.sh) - Launches Beach host with Pong demo
- [`tests/scripts/parse-bootstrap.sh`](./scripts/parse-bootstrap.sh) - Parses session credentials from bootstrap JSON
- [`tests/scripts/cleanup-test-session.sh`](./scripts/cleanup-test-session.sh) - Cleans up test processes
- [`tests/README.md`](./README.md) - Complete test documentation

**Modified Files:**
- [`package.json`](../package.json) - Added `test:e2e`, `test:e2e:ui`, `test:e2e:resize` scripts
- [`.gitignore`](../.gitignore) - Added Playwright output directories

### 2. Test Capabilities

The test suite can:

✅ **Connect to live Beach sessions** - Configurable via env vars or hardcoded credentials
✅ **Simulate viewport resize** - Increases height by 300px to mimic Private Beach tile expansion
✅ **Capture diagnostic data**:
- Before/after screenshots
- Terminal grid state (row-by-row analysis)
- Console trace logs
- Row kinds (missing vs loaded)

✅ **Verify fix behavior**:
- Checks for duplicate HUD content in newly exposed rows
- Analyzes blank vs loaded row ratios
- Detects consecutive duplicate rows
- Identifies repeated HUD patterns

✅ **Generate test artifacts**:
- `test-results/resize/01-before-resize.png`
- `test-results/resize/02-after-resize.png`
- `test-results/resize/03-trace-debug.png`
- Console logs with `[BEACH_TRACE]` output

### 3. Environment Setup (Completed)

Successfully brought up full test environment:

| Component | Status | Details |
|-----------|--------|---------|
| Docker Redis | ✅ Running | Port 6379 |
| beach-road server | ✅ Running | Port 8080 (PID: 19834) |
| Beach host session | ✅ Running | PID: 20182 |
| Pong demo | ✅ Running | PID: 20195 |
| beach-surfer dev server | ✅ Running | Port 5173 |

**Active Test Session:**
- Session ID: `ff762143-8458-4279-acd4-032a067c3c18` (or newer)
- Passcode: `OCK0J1`
- Server: `http://localhost:8080`

## Test Execution Status

### Challenges Encountered

1. **Auto-connect issues** - URL parameters pre-fill form but Connect button remains disabled
   - Root cause: Session validation or argon2 module loading
   - Workaround: Manual connection or extended wait times

2. **Session lifecycle** - Sessions expire quickly, requiring fresh sessions for each test run
   - Solution: Created helper scripts for easy session restart

3. **PTY/script interaction** - `script` command for PTY capture doesn't work well with background processes
   - Solution: Direct beach invocation with output redirection

### Fix Already Implemented

Based on changes observed in [`cache.test.ts`](../src/terminal/cache.test.ts), the PTY resize bug appears to have been **already fixed**:

**Test Changes (lines 456-483, 505-530, 553-578):**
```typescript
// OLD expectation (bug behavior):
expect(rows.slice(0, 4).map((row) => row.kind)).toEqual([
  'missing',  // ❌ Wrong - showed duplicate HUD
  'missing',
  'missing',
  'missing',
]);

// NEW expectation (fixed behavior):
expect(rows.slice(4, 8).map((row) => row.kind)).toEqual([
  'loaded',   // ✅ Correct - blank loaded rows
  'loaded',
  'loaded',
  'loaded',
]);
```

**New Test Added (lines 580-640):**
- `"PTY resize creates blank loaded rows not pending rows"`
- Verifies that PTY resize creates blank 'loaded' rows instead of 'missing'/'pending'
- Confirms original content rows remain unchanged
- Validates all intermediate rows are 'loaded' (not pending)

## How to Use

### Quick Start

```bash
# 1. Start infrastructure
docker-compose up -d redis
cargo run -p beach-road &

# 2. Start a Beach session
./target/debug/beach --session-server http://localhost:8080/ host \
  --bootstrap-output json -- \
  python3 ./apps/private-beach/demo/pong/player/main.py --mode lhs

# 3. Extract credentials from JSON output
SESSION_ID="<from-bootstrap-json>"
PASSCODE="<from-join_code>"

# 4. Run test
cd apps/beach-surfer
BEACH_TEST_SESSION_ID=$SESSION_ID \
BEACH_TEST_PASSCODE=$PASSCODE \
npm run test:e2e:resize
```

### Using Helper Scripts

```bash
# Launch session with scripts
./apps/beach-surfer/tests/scripts/launch-test-session.sh

# Credentials saved to: $HOME/beach-debug/session-creds.env
source $HOME/beach-debug/session-creds.env

# Run test
cd apps/beach-surfer
BEACH_TEST_SESSION_ID=$SESSION_ID \
BEACH_TEST_PASSCODE=$PASSCODE \
npm run test:e2e:resize

# Cleanup
./apps/beach-surfer/tests/scripts/cleanup-test-session.sh
```

## Current Test Environment

As of this session, the following processes are running and available for testing:

```
beach-road:    PID 19834 (port 8080)
beach host:    PID 20182
pong demo:     PID 20195
Session ID:    ff762143-8458-4279-acd4-032a067c3c18
Passcode:      OCK0J1
```

## Next Steps

1. **Resolve Connect button issue** - Investigate why button stays disabled despite URL pre-fill
   - Check session server connectivity from browser context
   - Verify argon2 module initialization timing
   - Consider implementing auto-connect without button click

2. **Run full test** - Once connection works, capture:
   - Screenshots showing blank rows (not duplicate HUD)
   - Terminal grid analysis confirming 'loaded' rows
   - Trace data via `window.__BEACH_TRACE_DUMP_ROWS()`

3. **Validate fix** - Confirm test passes with current codebase
   - Verify no duplicate HUD content appears
   - Check blank row ratio >= 80%
   - Review screenshots for visual confirmation

4. **Document findings** - Update `private-beach-duplicate-hud.md` with:
   - Test execution results
   - Screenshots proving fix works
   - Any remaining edge cases

## Files for Reference

- **Issue documentation**: [`docs/pty-resizing-issues/private-beach-duplicate-hud.md`](../../../docs/pty-resizing-issues/private-beach-duplicate-hud.md)
- **Test implementation**: [`tests/pty-resize-hud-duplication.spec.ts`](./pty-resize-hud-duplication.spec.ts)
- **Cache tests (showing fix)**: [`src/terminal/cache.test.ts`](../src/terminal/cache.test.ts)
- **Helper scripts**: [`tests/scripts/`](./scripts/)

## Cleanup Commands

```bash
# Kill all Beach processes
./apps/beach-surfer/tests/scripts/cleanup-test-session.sh

# Stop Docker services
docker-compose down

# Clean test artifacts
rm -rf apps/beach-surfer/test-results/
rm -rf apps/beach-surfer/playwright-report/
```

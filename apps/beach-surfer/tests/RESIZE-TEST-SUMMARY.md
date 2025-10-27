# PTY Resize Test - Implementation Summary

## What Was Built

A comprehensive test infrastructure for reproducing and diagnosing the PTY resizing HUD duplication issue documented in [`docs/pty-resizing-issues/private-beach-duplicate-hud.md`](../../../docs/pty-resizing-issues/private-beach-duplicate-hud.md).

## Files Created

### Test Files
- **`tests/pty-resize-hud-duplication.spec.ts`** - Playwright E2E test
  - Connects to live Beach sessions
  - Simulates viewport resize (300px height increase)
  - Captures screenshots before/after resize
  - Analyzes terminal grid for duplicate HUD content
  - Fails if duplicates detected

### Documentation
- **`tests/README.md`** - General E2E test documentation
- **`tests/RUN-RESIZE-TEST.md`** - Detailed manual test instructions
- **`tests/RESIZE-TEST-SUMMARY.md`** - This file

### Scripts
- **`tests/scripts/run-manual-resize-test.sh`** - Semi-automated test runner
  - Starts Redis, beach-road, Beach host session, and vite dev server
  - Displays session credentials
  - Provides step-by-step manual test instructions
  - Handles cleanup on Ctrl+C

- **`tests/scripts/cleanup-resize-test.sh`** - Cleanup all test processes
  - Kills beach-road, beach host, vite
  - Stops Docker containers
  - Removes temp files

- **`tests/scripts/launch-test-session.sh`** - Launch Beach session with Pong
  - Uses `script` command to capture PTY output
  - Extracts session credentials from bootstrap JSON
  - (Note: Has issues with background execution)

- **`tests/scripts/parse-bootstrap.sh`** - Parse bootstrap JSON for credentials
  - Extracts session_id and join_code
  - Outputs in shell-sourceable format

### Configuration
- **`package.json`** - Added npm scripts:
  - `test:e2e` - Run all Playwright tests
  - `test:e2e:ui` - Interactive Playwright UI
  - `test:e2e:resize` - Run only resize test

- **`.gitignore`** - Excluded Playwright output directories

## Current Status

### ✅ Working
- Infrastructure setup (Redis, beach-road, Beach host)
- Session creation and credential extraction
- Manual testing workflow
- Screenshot capture
- Terminal grid state analysis
- Cleanup automation

### ⚠️ Known Limitations
1. **Automated connection not working**
   - URL query parameters (`?session=...&passcode=...`) don't pre-fill the form in beach-surfer
   - Connect button remains disabled even with correct credentials
   - Requires manual form entry and button click

2. **Script command issues**
   - `launch-test-session.sh` has problems with background PTY capture
   - Bootstrap JSON extraction works but process management is flaky

3. **Test requires manual interaction**
   - Cannot fully automate end-to-end without fixing URL parameter handling
   - Best approach is semi-automated: script sets up environment, human performs test

## Recommended Usage

### Quick Start
```bash
cd /Users/arellidow/development/beach/apps/beach-surfer
./tests/scripts/run-manual-resize-test.sh
```

This will:
1. Start all infrastructure
2. Create a Beach session with Pong
3. Display session credentials
4. Print step-by-step test instructions
5. Wait for Ctrl+C to cleanup

### Manual Cleanup
```bash
./tests/scripts/cleanup-resize-test.sh
```

## Test Procedure

1. **Start infrastructure** (automated via script)
2. **Open browser** to http://localhost:5173
3. **Enter credentials** manually
4. **Click Connect** and wait for terminal to load
5. **Observe Pong HUD** (should see commands, mode, prompt)
6. **Resize browser** window taller by dragging corner
7. **Check newly exposed rows**:
   - ✅ **Expected**: Blank rows (fix working)
   - ❌ **Bug**: Duplicate HUD content (fix not working)

## Debug Tools

### Browser Console
```javascript
// Enable trace logging
window.__BEACH_TRACE = true

// Dump terminal grid state
window.__BEACH_TRACE_DUMP_ROWS(20)

// View last dump
window.__BEACH_TRACE_LAST_ROWS

// Access terminal cache
window.__terminalCache.snapshot()
window.__terminalCache.visibleRows()
```

## Future Improvements

To make this fully automated:

1. **Fix beach-surfer URL parameter handling**
   - Investigate why `?session=...&passcode=...` doesn't pre-fill form
   - Add query parameter parsing in App.tsx
   - Auto-enable Connect button when valid credentials provided via URL

2. **Add programmatic connection API**
   - Expose transport connection methods to `window` object for testing
   - Allow Playwright to call `window.connectToSession(id, passcode)` directly
   - Bypass UI validation entirely

3. **Improve session lifecycle management**
   - Better background process handling in launch script
   - Health checks before running test
   - Automatic retry on transient failures

## Test Artifacts

When tests run, artifacts are saved to:
```
apps/beach-surfer/test-results/
├── resize/
│   ├── 01-before-resize.png
│   ├── 02-after-resize.png
│   └── 03-trace-debug.png
└── pty-resize-hud-duplication-*/
    ├── test-failed-1.png
    └── error-context.md
```

## References

- Original issue: `docs/pty-resizing-issues/private-beach-duplicate-hud.md`
- Cache implementation: `apps/beach-surfer/src/terminal/cache.ts`
- Cache tests: `apps/beach-surfer/src/terminal/cache.test.ts`
- BeachTerminal component: `apps/beach-surfer/src/components/BeachTerminal.tsx`

## Notes

- This test infrastructure was built with the constraint that Beach sessions require a live PTY and real-time interaction
- The semi-automated approach (script setup + manual testing) is actually more practical than a fully automated test for this use case
- The test scripts provide excellent reproducibility and documentation, even if they require manual steps
- All test utilities are reusable for other Beach integration tests

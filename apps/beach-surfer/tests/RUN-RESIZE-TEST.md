# Running the PTY Resize Test

## Current Test Infrastructure Status

✅ **Created:**
- Automated test framework (`pty-resize-hud-duplication.spec.ts`)
- Session management scripts (`launch-test-session.sh`, `cleanup-test-session.sh`)
- Environment setup and teardown
- Screenshot and trace data capture

⚠️ **Known Issues:**
- URL query parameters (`?session=...&passcode=...`) don't pre-fill the beach-surfer connection form
- Automated test can't click the Connect button (remains disabled until manual interaction)

## Recommended Approach: Semi-Automated Testing

### Quick Test Run

```bash
# Terminal 1: Start infrastructure and Beach session
cd /Users/arellidow/development/beach

# Start Redis + beach-road
docker-compose up -d redis
cargo run -p beach-road &

# Start Beach host session with Pong (captures bootstrap JSON)
env BEACH_LOG_LEVEL=warn \
  ./target/debug/beach \
  --session-server http://localhost:8080/ \
  host \
  --bootstrap-output json \
  --wait \
  -- /usr/bin/env python3 \
    /Users/arellidow/development/beach/apps/private-beach/demo/pong/player/main.py \
    --mode lhs \
  > /tmp/beach-bootstrap.txt 2>&1 &

# Extract credentials
cat /tmp/beach-bootstrap.txt | head -1
# Copy the session_id and join_code values

# Terminal 2: Start beach-surfer dev server
cd apps/beach-surfer
npm run dev

# Terminal 3: Open beach-surfer in browser and connect manually
open http://localhost:5173
# Fill in session ID and passcode from bootstrap output
# Click Connect
# Once connected, manually resize the browser window (drag corner)
# Observe if HUD duplicates when viewport expands
```

### Current Test Session

**Active session details** (if still running):
```
Session ID: ff762143-8458-4279-acd4-032a067c3c18
Passcode:   OCK0J1
Server:     http://localhost:8080/
PID:        27414
```

Check if still alive:
```bash
ps aux | grep "27414"
```

## What to Look For

### Expected Behavior (Fix Working)
When you resize the browser window taller:
- ✅ Newly exposed rows at the top should be **blank**
- ✅ No duplicate HUD content ("Unknown command", "Commands", "Mode", ">")
- ✅ Terminal grid properly pads with `MissingRow` entries

### Bug Behavior (Fix Not Working)
When you resize the browser window taller:
- ❌ Newly exposed rows show **duplicate HUD text**
- ❌ You see repeated "Unknown command", "Commands", "Mode", ">" lines
- ❌ Content appears to be "copied" to fill the new space

## Debug Tools

### Enable Beach Trace Logging

In browser console (before connecting):
```javascript
window.__BEACH_TRACE = true
```

### Capture Terminal Grid State

After resizing:
```javascript
window.__BEACH_TRACE_DUMP_ROWS(20)  // Dump first 20 rows
window.__BEACH_TRACE_LAST_ROWS       // View last dump
```

### Check Terminal Cache

```javascript
// In browser console
const cache = window.__terminalCache
cache.snapshot()           // Full state
cache.visibleRows()        // Current visible rows
```

## Test Artifacts

Screenshots and logs are saved to:
```
apps/beach-surfer/test-results/resize/
├── 01-before-resize.png
├── 02-after-resize.png
└── 03-trace-debug.png
```

## Cleanup

```bash
# Kill all test processes
pkill -f "beach.*host"
pkill -f "python.*pong"
pkill -f "beach-road"

# Stop Docker
docker-compose down

# Or use cleanup script
./apps/beach-surfer/tests/scripts/cleanup-test-session.sh
```

## Troubleshooting

### "Connect button is disabled"
- **Cause**: Session server URL mismatch or session expired
- **Fix**:
  1. Check beach-road is running: `curl http://localhost:8080/health`
  2. Verify session exists: `curl http://localhost:8080/sessions/<session-id>`
  3. Start a fresh session (sessions may have short TTL)

### "Terminal not visible after connect"
- **Cause**: Session closed or connection failed
- **Fix**:
  1. Check browser console for errors
  2. Verify Beach host process is still running
  3. Check beach-road logs for connection messages

### "No HUD visible to duplicate"
- **Cause**: Pong may not have initialized
- **Fix**: Wait 3-5 seconds after session start for Pong to render HUD

## Future Improvements

To make this fully automated, we need:
1. Fix URL query parameter handling in beach-surfer App.tsx
2. Add programmatic "Connect" button enable check (wait for validation to complete)
3. Or use Playwright's `page.evaluate()` to bypass the UI and connect directly via transport APIs

## Test Results Documentation

Record your findings:
```
Date: ____________________
Session ID: ______________
Passcode: ________________

Resize Test Results:
[ ] Pass - No HUD duplication observed
[ ] Fail - HUD duplicated when resizing taller

Notes:
_______________________________
_______________________________
_______________________________

Screenshots attached: Yes / No
```

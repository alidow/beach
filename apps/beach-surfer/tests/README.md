# Beach Surfer E2E Tests

This directory contains Playwright end-to-end tests for beach-surfer. These tests are **manual/on-demand** and require a live Beach session.

## Tests

### PTY Resize HUD Duplication (`pty-resize-hud-duplication.spec.ts`)

Reproduces and verifies the fix for the HUD duplication issue documented in [`docs/pty-resizing-issues/private-beach-duplicate-hud.md`](../../../docs/pty-resizing-issues/private-beach-duplicate-hud.md).

**What it tests:**
- Connects to a live Beach session (Pong demo or Private Beach)
- Simulates viewport resize (mimicking tile expansion)
- Verifies that newly exposed rows render as blank instead of duplicate HUD content
- Captures diagnostic data including screenshots and trace logs

## Setup

### Prerequisites

1. **Install Playwright** (if not already installed):
   ```bash
   npx playwright install chromium
   ```

2. **Start a Beach session:**
   ```bash
   # Option 1: Pong demo via Private Beach
   cd apps/private-beach/demo/pong
   python3 tools/launch_session.py

   # Option 2: Direct Beach session
   cargo run -p beach
   ```

3. **Note the session ID and passcode** from the terminal output

### Running the Tests

#### Method 1: Environment Variables (Recommended)

```bash
cd apps/beach-surfer

# Run with env vars
BEACH_TEST_SESSION_ID=your-session-id \
BEACH_TEST_PASSCODE=your-passcode \
npm run test:e2e:resize
```

#### Method 2: Hardcode in Test File

Edit `tests/pty-resize-hud-duplication.spec.ts` and update:
```typescript
const SESSION_ID = process.env.BEACH_TEST_SESSION_ID || 'your-session-id';
const PASSCODE = process.env.BEACH_TEST_PASSCODE || 'your-passcode';
```

Then run:
```bash
npm run test:e2e:resize
```

#### Method 3: Interactive Mode

For debugging with Playwright UI:
```bash
BEACH_TEST_SESSION_ID=your-session-id \
BEACH_TEST_PASSCODE=your-passcode \
npm run test:e2e:ui
```

## Configuration

Environment variables for customization:

| Variable | Default | Description |
|----------|---------|-------------|
| `BEACH_TEST_SESSION_ID` | _(required)_ | Session ID to connect to |
| `BEACH_TEST_PASSCODE` | _(required)_ | Session passcode |
| `BEACH_TEST_SESSION_SERVER` | `http://localhost:4132` | beach-road server URL |
| `BEACH_TEST_URL` | `http://localhost:5173` | beach-surfer dev server URL |
| `BEACH_TEST_SKIP_CONNECT` | `false` | Skip auto-connect (manual mode) |

## Test Outputs

Test artifacts are saved to `test-results/`:

- **Screenshots:**
  - `01-before-resize.png` - Initial terminal state
  - `02-after-resize.png` - After viewport expansion
  - `03-trace-debug.png` - Debug test screenshot

- **Console logs** - All `[BEACH_TRACE]` output captured
- **Trace data** - Row-by-row grid state via `window.__BEACH_TRACE_DUMP_ROWS()`

## Example Session

```bash
# Terminal 1: Start Pong demo
cd apps/private-beach/demo/pong
python3 tools/launch_session.py

# Output will include:
# Session ID: 55de4d80-8a0e-4fda-b885-97515fb08eb6
# Join code: JITQ3N

# Terminal 2: Run test
cd apps/beach-surfer
BEACH_TEST_SESSION_ID=55de4d80-8a0e-4fda-b885-97515fb08eb6 \
BEACH_TEST_PASSCODE=JITQ3N \
npm run test:e2e:resize
```

## Troubleshooting

### "Connect button is disabled"
- Ensure beach-road server is running on the correct port (default: 4132)
- Check that the session ID is still valid (sessions may expire)
- Wait 2-3 seconds for argon2 WASM module to load

### "Terminal not found"
- Verify beach-surfer dev server is running (`npm run dev`)
- Check that the session is active and hasn't closed

### "Test timeout"
- Increase timeout with `--timeout=60000` flag
- Check network connectivity to session server
- Verify session credentials are correct

## CI/CD

These tests are **not run in CI** by default, as they require:
- A live Beach session
- Manual session ID/passcode configuration
- Active beach-road server

They are intended for:
- Manual regression testing during development
- Reproducing reported issues
- Validating fixes before deployment
- Debugging session-specific conditions

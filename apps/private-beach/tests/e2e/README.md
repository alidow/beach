# Private Beach E2E Tests

Playwright end-to-end tests for Private Beach, focusing on terminal tile behavior and PTY resize issues.

## Test Files

- **`tile-resize-hud.spec.ts`** - Tests PTY resize behavior in Private Beach tiles, specifically checking for duplicate HUD content after tile expansion

## Setup

### Prerequisites

1. **Install Playwright browsers** (first time only):
   ```bash
   cd apps/private-beach
   npx playwright install
   ```

2. **Install dependencies**:
   ```bash
   npm install
   ```

### Infrastructure Requirements

The tests require the following services to be running:

- **Redis** (for session storage)
- **beach-road** (session server on port 8080)
- **Beach host session** (with Pong demo or other TUI app)
- **Private Beach Next.js app** (on port 3000)

## Running Tests

### Automated (Recommended)

Use the automation script to start all infrastructure and run tests:

```bash
cd apps/private-beach
./tests/scripts/run-tile-resize-test.sh
```

**Note**: The automation script currently requires a manual step:
1. It will start all services and create a Beach session
2. You must manually create a beach in the UI and add the session
3. Set the `BEACH_ID` environment variable
4. Press Enter to continue with the test

### Manual

If you prefer to manage infrastructure yourself:

1. **Start services**:
   ```bash
   # Terminal 1: Redis
   docker-compose up -d redis

   # Terminal 2: beach-road
   BEACH_PUBLIC_SESSION_SERVER=http://localhost:8080 cargo run -p beach-road

   # Terminal 3: Beach host with Pong
   cargo run -p beach -- host --session-server http://localhost:8080 -- \
     python3 apps/private-beach/demo/pong/player/main.py --mode lhs

   # Terminal 4: Private Beach
   cd apps/private-beach
   npm run dev
   ```

2. **Note the session credentials** from Beach host output

3. **Create a beach and add the session** via Private Beach UI

4. **Run the test**:
   ```bash
   cd apps/private-beach
   BEACH_ID=<beach-id> \
   BEACH_TEST_SESSION_ID=<session-id> \
   BEACH_TEST_PASSCODE=<passcode> \
   npm run test:e2e:resize
   ```

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `BEACH_TEST_SESSION_ID` | Beach session ID | (required) |
| `BEACH_TEST_PASSCODE` | Session join passcode | (required) |
| `BEACH_TEST_SESSION_SERVER` | Session server URL | `http://localhost:8080` |
| `BEACH_ID` | Private Beach dashboard ID | (required) |
| `BEACH_TEST_MANAGER_URL` | Manager API URL | `http://localhost:8080` |
| `BEACH_TEST_PRIVATE_BEACH_URL` | Private Beach URL | `http://localhost:3000` |

## Test Scripts

Available npm scripts:

```bash
npm run test:e2e              # Run all E2E tests
npm run test:e2e:resize       # Run only tile resize tests
npm run test:e2e:ui           # Run with Playwright UI
npm run test:e2e:headed       # Run in headed mode (see browser)
```

## Debugging

### Enable Trace Logging

The tests automatically enable Beach trace logging (`window.__BEACH_TRACE = true`). This provides detailed console output about terminal state.

### Screenshots

Failed tests automatically capture screenshots in:
```
apps/private-beach/test-results/
```

### Manual Inspection

Run with Playwright UI for step-by-step debugging:
```bash
npm run test:e2e:ui
```

### Console Output

The tests log detailed information about:
- Initial terminal state (rows, content)
- Tile resize dimensions
- Terminal state after resize
- Duplicate content analysis

## Test Architecture

### Helpers

- **`helpers/terminal-capture.ts`** - Captures terminal state using Beach trace API
- **`helpers/tile-manipulation.ts`** - Interacts with react-grid-layout tiles
- **`fixtures/beach-session.ts`** - Manages Beach session credentials

### Test Flow

1. Enable trace logging
2. Navigate to Private Beach dashboard
3. Locate tile by session ID
4. Wait for terminal connection
5. Capture initial terminal state
6. Resize tile to ~70 rows × 90 cols (via dragging resize handle)
7. Wait 5 seconds for PTY backfill/replay
8. Capture post-resize terminal state
9. Analyze for duplicate HUD content
10. Assert no duplicates found

## Known Issues

### Manual Beach Creation Required

The current implementation requires manually creating a beach and adding the session via the UI. Future improvements:

- Automate beach creation via API calls
- Automatically add session to beach via API
- Eliminate need for `BEACH_ID` environment variable

### Tile Resize Limitations

The tile resize helper uses drag interactions which may be sensitive to:
- Screen resolution
- Browser window size
- react-grid-layout configuration changes

If resize fails, check the console output for calculated drag distances.

## Cleanup

To stop all test infrastructure:

```bash
cd apps/private-beach
./tests/scripts/cleanup-test-env.sh
```

This stops:
- Beach host session
- beach-road server
- Private Beach dev server
- Redis Docker container
- Cleans up temporary files

## Related Documentation

- [PTY Resizing Issues Documentation](../../../../docs/pty-resizing-issues/private-beach-duplicate-hud.md)
- [beach-surfer PTY Resize Test](../../../beach-surfer/tests/pty-resize-hud-duplication.spec.ts)

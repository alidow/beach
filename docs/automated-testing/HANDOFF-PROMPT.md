# Private Beach E2E Test Implementation - Handoff Prompt

## For: Next Claude Code Instance

Copy this entire prompt to implement fully automated Private Beach E2E tests.

---

# Task: Implement Fully Automated Private Beach E2E Test Infrastructure

## Context

You're working on the Beach codebase (terminal sharing application). A PTY resize bug has been fixed in `apps/beach-surfer/src/terminal/cache.ts`, and unit tests + standalone E2E tests pass. However, the Private Beach tile resize E2E test requires full infrastructure setup to run.

## Your Mission

Implement complete automation for the Private Beach tile resize E2E test so it can run with a single command.

## What Already Exists

### ✅ Completed
- Bug fix in `apps/beach-surfer/src/terminal/cache.ts:setGridSize()`
- Unit tests passing (apps/beach-surfer/src/terminal/cache.test.ts)
- Standalone E2E test passing (apps/beach-surfer/tests/pty-resize-standalone.spec.ts)
- Playwright test file (apps/private-beach/tests/e2e/tile-resize-hud.spec.ts)
- Test helper files in apps/private-beach/tests/e2e/helpers/
- Clerk test account: `testuser` / `beach r0cks!`

### 📋 Documentation to Read First
1. `docs/automated-testing/private-beach-e2e-infrastructure.md` - Infrastructure setup guide (just created)
2. `docs/pty-resizing-issues/test-execution-and-diagnostics.md` - Test execution details
3. `apps/private-beach/tests/e2e/README.md` - Test README

## Tasks to Complete

### 1. Fix beach-road Port Configuration ⚠️ CRITICAL

**Problem:** beach-road hardcodes port 8080, but beach-manager also uses 8080

**Solution:** Modify beach-road to read PORT or BEACH_ROAD_PORT environment variable

**Files to modify:**
- `apps/beach-road/src/main.rs` or relevant config file

**Implementation:**
```rust
// Find where port is set (likely in main.rs)
// Change from hardcoded 8080 to:
let port = std::env::var("BEACH_ROAD_PORT")
    .or_else(|_| std::env::var("PORT"))
    .unwrap_or_else(|_| "4132".to_string());
let addr = format!("0.0.0.0:{}", port);
```

**Verify:**
```bash
BEACH_ROAD_PORT=4132 cargo run -p beach-road
# Should listen on 0.0.0.0:4132
curl http://localhost:4132/health
```

### 2. Create Clerk Authentication Helper

**File:** `apps/private-beach/tests/e2e/helpers/clerk-auth.ts`

**Functions needed:**
```typescript
export async function signInWithClerk(page: Page, username: string, password: string): Promise<void>;
export async function getClerkToken(page: Page, template?: string): Promise<string>;
export async function saveAuthState(page: Page, path: string): Promise<void>;
export async function loadAuthState(page: Page, path: string): Promise<void>;
```

**Implementation details:**
- Navigate to /sign-in
- Fill username field, click Continue
- Fill password field, click Continue
- Wait for redirect to /beaches
- Extract token via: `await window.Clerk.session.getToken({ template: 'private-beach-manager' })`

**Clerk credentials:**
- Username: `testuser`
- Password: `beach r0cks!`
- Template: `private-beach-manager`

### 3. Create Beach Setup Helper

**File:** `apps/private-beach/tests/e2e/helpers/beach-setup.ts`

**Functions needed:**
```typescript
export async function createBeach(token: string, name: string, slug?: string): Promise<{ id: string; name: string; slug: string }>;
export async function attachSessionToBeach(token: string, beachId: string, sessionId: string, passcode: string): Promise<void>;
export async function deleteBeach(token: string, beachId: string): Promise<void>;
```

**API Endpoints:**
- POST `http://localhost:8080/private-beaches` - Create beach
- POST `http://localhost:8080/private-beaches/{id}/sessions/attach-by-code` - Attach session
- Headers: `Authorization: Bearer ${token}`, `Content-Type: application/json`

**Example:**
```typescript
const response = await fetch('http://localhost:8080/private-beaches', {
  method: 'POST',
  headers: {
    'Authorization': `Bearer ${token}`,
    'Content-Type': 'application/json',
  },
  body: JSON.stringify({ name, slug }),
});
const beach = await response.json();
return beach;
```

### 4. Create Infrastructure Startup Script

**File:** `scripts/start-private-beach-tests.sh`

**Requirements:**
1. Start Docker containers (postgres, redis)
2. Wait for databases to be ready
3. Run database migrations
4. Start beach-road on port 4132
5. Start beach-manager on port 8080
6. Start Beach host session with Pong
7. Extract session credentials
8. Start Private Beach dev server
9. Output all credentials for test

**Script structure:**
```bash
#!/usr/bin/env bash
set -euo pipefail

echo "🏖️  Starting Private Beach Test Infrastructure"

# 1. Docker services
docker-compose up -d postgres redis
echo "Waiting for databases..."
timeout 30 bash -c 'until docker exec beach-postgres pg_isready; do sleep 1; done'
timeout 30 bash -c 'until docker exec beach-redis redis-cli ping; do sleep 1; done'

# 2. Run migrations
export DATABASE_URL="postgres://postgres:postgres@localhost:5432/beach_manager"
cd apps/beach-manager
sqlx migrate run --source migrations
cd ../..

# 3. Start beach-road on 4132
export BEACH_ROAD_PORT=4132
export REDIS_URL="redis://localhost:6379"
cargo run -p beach-road > /tmp/beach-road.log 2>&1 &
BEACH_ROAD_PID=$!
echo $BEACH_ROAD_PID > /tmp/beach-road.pid

# Wait and verify
sleep 3
curl -sf http://localhost:4132/health || { echo "beach-road failed"; exit 1; }

# 4. Start beach-manager on 8080
export DATABASE_URL="postgres://postgres:postgres@localhost:5432/beach_manager"
cargo run -p beach-manager > /tmp/beach-manager.log 2>&1 &
BEACH_MANAGER_PID=$!
echo $BEACH_MANAGER_PID > /tmp/beach-manager.pid

# Wait and verify
sleep 3
curl -sf http://localhost:8080/healthz || { echo "beach-manager failed"; exit 1; }

# 5. Start Beach host
cargo run -p beach -- host \
  --session-server http://localhost:4132 \
  --bootstrap-output json \
  -- python3 apps/private-beach/demo/pong/player/main.py --mode lhs \
  > /tmp/beach-bootstrap.txt 2>&1 &
BEACH_HOST_PID=$!
echo $BEACH_HOST_PID > /tmp/beach-host.pid

sleep 5

# Extract credentials
SESSION_ID=$(grep -o '"session_id":"[^"]*"' /tmp/beach-bootstrap.txt | cut -d'"' -f4)
PASSCODE=$(grep -o '"join_code":"[^"]*"' /tmp/beach-bootstrap.txt | cut -d'"' -f4)

echo "Session ID: $SESSION_ID"
echo "Passcode: $PASSCODE"

# Export for tests
export BEACH_TEST_SESSION_ID="$SESSION_ID"
export BEACH_TEST_PASSCODE="$PASSCODE"
export BEACH_TEST_SESSION_SERVER="http://localhost:4132"
export BEACH_TEST_MANAGER_URL="http://localhost:8080"

# 6. Start Private Beach (if not running)
if ! lsof -ti:3000 > /dev/null 2>&1; then
  cd apps/private-beach
  npm run dev > /tmp/private-beach.log 2>&1 &
  NEXT_PID=$!
  echo $NEXT_PID > /tmp/private-beach.pid
  cd ../..
  sleep 5
fi

echo "✅ Infrastructure ready!"
echo "Run tests with: cd apps/private-beach && npm run test:e2e:resize"
```

### 5. Create Infrastructure Cleanup Script

**File:** `scripts/stop-private-beach-tests.sh`

```bash
#!/usr/bin/env bash

echo "🧹 Stopping Private Beach Test Infrastructure"

# Kill processes
for pidfile in /tmp/beach-road.pid /tmp/beach-manager.pid /tmp/beach-host.pid /tmp/private-beach.pid; do
  if [ -f "$pidfile" ]; then
    kill $(cat "$pidfile") 2>/dev/null || true
    rm "$pidfile"
  fi
done

# Stop Docker
docker-compose stop postgres redis

echo "✅ Cleanup complete"
```

### 6. Update Test to Use Automation

**File:** `apps/private-beach/tests/e2e/tile-resize-hud.spec.ts`

**Modify the test to:**

1. Remove manual beach creation requirement
2. Use helper functions to automate setup
3. Clean up beach after test

**Example structure:**
```typescript
import { signInWithClerk, getClerkToken } from './helpers/clerk-auth';
import { createBeach, attachSessionToBeach, deleteBeach } from './helpers/beach-setup';

test.describe('Private Beach Tile Resize - Automated', () => {
  let beachId: string;
  let authToken: string;

  test.beforeAll(async ({ browser }) => {
    // Create separate page for setup
    const page = await browser.newPage();

    // Sign in and get token
    await signInWithClerk(page, 'testuser', 'beach r0cks!');
    authToken = await getClerkToken(page, 'private-beach-manager');

    // Create beach
    const session = createSessionMetadata();
    const beach = await createBeach(authToken, 'Test Beach', 'test-' + Date.now());
    beachId = beach.id;

    // Attach session
    await attachSessionToBeach(
      authToken,
      beachId,
      session.credentials.sessionId,
      session.credentials.passcode
    );

    await page.close();
  });

  test.afterAll(async () => {
    // Cleanup: delete beach
    if (beachId && authToken) {
      await deleteBeach(authToken, beachId);
    }
  });

  test('should not duplicate HUD when tile resized', async ({ page }) => {
    // Sign in
    await signInWithClerk(page, 'testuser', 'beach r0cks!');

    // Navigate to beach
    await page.goto(`http://localhost:3000/beaches/${beachId}`);

    // Rest of existing test logic...
    // (tile finding, resizing, duplicate detection)
  });
});
```

### 7. Create npm Test Script

**File:** `apps/private-beach/package.json`

Add or update scripts:
```json
{
  "scripts": {
    "test:e2e:resize": "playwright test tile-resize-hud.spec.ts --workers=1",
    "test:e2e:resize:ui": "playwright test tile-resize-hud.spec.ts --ui",
    "test:e2e:resize:debug": "PWDEBUG=1 playwright test tile-resize-hud.spec.ts"
  }
}
```

### 8. Create Master Test Runner Script

**File:** `scripts/run-private-beach-e2e-tests.sh`

```bash
#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "🏖️  Private Beach E2E Test Suite"
echo "================================"

# Start infrastructure
"$SCRIPT_DIR/start-private-beach-tests.sh"

# Run tests
cd apps/private-beach
npm run test:e2e:resize -- --workers=1 --timeout=90000

TEST_EXIT_CODE=$?

# Cleanup
cd ../..
"$SCRIPT_DIR/stop-private-beach-tests.sh"

if [ $TEST_EXIT_CODE -eq 0 ]; then
  echo "✅ All tests passed!"
else
  echo "❌ Tests failed"
  exit $TEST_EXIT_CODE
fi
```

## Acceptance Criteria

When complete, this command should work from a fresh state:

```bash
./scripts/run-private-beach-e2e-tests.sh
```

And should:
1. ✅ Start all infrastructure services
2. ✅ Create Beach host session with Pong
3. ✅ Sign into Private Beach with Clerk
4. ✅ Create a beach via API
5. ✅ Attach session to beach via API
6. ✅ Navigate to beach in browser
7. ✅ Resize tile significantly (70+ rows)
8. ✅ Verify 0 duplicate HUD content
9. ✅ Take screenshots
10. ✅ Clean up all services

## Verification Steps

1. Run the master script:
   ```bash
   ./scripts/run-private-beach-e2e-tests.sh
   ```

2. Check test output for:
   ```
   Initial state: { viewportRows: 24, ... }
   After resize: { viewportRows: 70, ... }
   ✓ No duplicate HUD content detected
   Test passed
   ```

3. Verify cleanup:
   ```bash
   lsof -i :8080  # Should be empty
   lsof -i :4132  # Should be empty
   docker ps | grep beach  # Should show stopped containers
   ```

## Important Notes

- Clerk credentials: `testuser` / `beach r0cks!`
- Session server: http://localhost:4132 (beach-road)
- Manager API: http://localhost:8080 (beach-manager)
- Private Beach: http://localhost:3000
- Database: postgres://postgres:postgres@localhost:5432/beach_manager
- Redis: redis://localhost:6379

## Files You'll Create/Modify

**New files:**
- `apps/private-beach/tests/e2e/helpers/clerk-auth.ts`
- `apps/private-beach/tests/e2e/helpers/beach-setup.ts`
- `scripts/start-private-beach-tests.sh`
- `scripts/stop-private-beach-tests.sh`
- `scripts/run-private-beach-e2e-tests.sh`

**Modified files:**
- `apps/beach-road/src/main.rs` (or config file) - Add PORT configuration
- `apps/private-beach/tests/e2e/tile-resize-hud.spec.ts` - Use automation helpers
- `apps/private-beach/package.json` - Add test scripts

## Success Metrics

The implementation is complete when:
1. ✅ Single command runs entire test suite
2. ✅ No manual intervention required
3. ✅ Test passes with 0 duplicates
4. ✅ All services start and stop cleanly
5. ✅ Can run repeatedly without errors

## Troubleshooting Tips

If you encounter issues:

1. **Port conflicts:** Check `lsof -i :8080` and `lsof -i :4132`
2. **Database issues:** Verify migrations ran with `psql $DATABASE_URL -c "\dt"`
3. **Service failures:** Check logs in `/tmp/beach-*.log`
4. **Clerk auth:** Ensure password is `beach r0cks!` (with space and exclamation)
5. **Test failures:** Run with `--ui` flag to see browser interaction

Good luck! You have all the context needed to complete this implementation.

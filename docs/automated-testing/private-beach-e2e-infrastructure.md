# Private Beach E2E Test Infrastructure Setup

## Overview

This document provides a complete plan for setting up the infrastructure needed to run automated Private Beach tile resize E2E tests using Playwright.

## Current Status

### ✅ Completed
- PTY resize bug fix implemented in `apps/beach-surfer/src/terminal/cache.ts`
- Unit tests passing (21/21)
- Standalone beach-surfer E2E test passing with 0 duplicates
- Clerk authentication working (testuser / beach r0cks!)
- Playwright test files created:
  - `apps/private-beach/tests/e2e/tile-resize-hud.spec.ts`
  - Helper files in `apps/private-beach/tests/e2e/helpers/`
  - Fixture files in `apps/private-beach/tests/e2e/fixtures/`

### ⚠️ Blocked
- Private Beach E2E test requires full infrastructure
- beach-manager not properly configured
- Port conflicts between beach-road and beach-manager

## Infrastructure Requirements

### Services Needed

1. **Redis** - Session storage
2. **PostgreSQL** - Beach manager database
3. **beach-road** - Session server (WebRTC signaling)
4. **beach-manager** - Private Beach control plane
5. **Beach host** - Terminal session host
6. **Private Beach** - Next.js web app

### Port Configuration

| Service | Port | Environment Variable |
|---------|------|---------------------|
| beach-manager | 8080 | `NEXT_PUBLIC_MANAGER_URL` |
| beach-road | 4132 | `NEXT_PUBLIC_ROAD_URL` |
| Private Beach | 3000/3001 | N/A (dev server) |
| Redis | 6379 | `REDIS_URL` |
| PostgreSQL | 5432 | `DATABASE_URL` |

## Setup Steps

### 1. Database Setup

```bash
# Start PostgreSQL and Redis via Docker
docker-compose up -d postgres redis

# Wait for services to be ready
sleep 5

# Verify PostgreSQL is running
docker exec beach-postgres pg_isready -U postgres

# Verify Redis is running
docker exec beach-redis redis-cli ping
```

### 2. Run Database Migrations

```bash
# Set DATABASE_URL
export DATABASE_URL="postgres://postgres:postgres@localhost:5432/beach_manager"

# Run migrations for beach-manager
cd apps/beach-manager
sqlx migrate run --source migrations

# Verify migrations succeeded
psql $DATABASE_URL -c "\dt"
```

### 3. Start beach-road (Session Server)

**Issue to Fix:** beach-road doesn't honor PORT environment variable

**Current Workaround:**
```bash
# beach-road currently hardcodes port 8080 in code
# Need to update beach-road to use configurable port
```

**Proper Solution:**
```bash
# Update beach-road/src/main.rs to read PORT or BEACH_ROAD_PORT env var
# Example code change needed:
# let port = std::env::var("BEACH_ROAD_PORT").unwrap_or_else(|_| "4132".to_string());
# let addr = format!("0.0.0.0:{}", port);

# Then start with:
export BEACH_ROAD_PORT=4132
export REDIS_URL="redis://localhost:6379"
cargo run -p beach-road
```

### 4. Start beach-manager (Control Plane)

```bash
export DATABASE_URL="postgres://postgres:postgres@localhost:5432/beach_manager"
export REDIS_URL="redis://localhost:6379"
export BEACH_MANAGER_PORT=8080  # Default

cargo run -p beach-manager
```

**Health Check:**
```bash
curl http://localhost:8080/healthz
# Should return: ok
```

### 5. Start Beach Host Session

```bash
# Start Beach host with Pong demo
cargo run -p beach -- host \
  --session-server http://localhost:4132 \
  --bootstrap-output json \
  -- python3 apps/private-beach/demo/pong/player/main.py --mode lhs

# Capture output to extract credentials
# Look for JSON line with:
# - session_id: (UUID)
# - join_code: (6-char code)
```

**Save Credentials:**
```bash
# Example output parsing
SESSION_ID=$(grep -o '"session_id":"[^"]*"' /tmp/beach-bootstrap.txt | cut -d'"' -f4)
PASSCODE=$(grep -o '"join_code":"[^"]*"' /tmp/beach-bootstrap.txt | cut -d'"' -f4)
```

### 6. Start Private Beach Web App

```bash
cd apps/private-beach
npm run dev

# Will start on port 3000 (or 3001 if 3000 is in use)
```

**Verify Environment:**
```bash
# Check .env.local has correct values:
cat apps/private-beach/.env.local

# Should contain:
# NEXT_PUBLIC_MANAGER_URL=http://localhost:8080
# NEXT_PUBLIC_ROAD_URL=http://localhost:4132
# NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY=pk_test_...
# CLERK_SECRET_KEY=sk_test_...
```

## Automated Test Execution

### Prerequisites

1. All services running (see above)
2. Beach session created with Pong
3. Clerk test account: `testuser` / `beach r0cks!`

### Method 1: Automated Beach Creation (Recommended)

Create a test setup script that:

1. **Authenticate with Clerk via Playwright**
   ```typescript
   // Save auth state for reuse
   await page.goto('http://localhost:3000/sign-in');
   await page.fill('input[name="identifier"]', 'testuser');
   await page.click('button:has-text("Continue")');
   await page.fill('input[name="password"]', 'beach r0cks!');
   await page.click('button:has-text("Continue")');
   await page.context().storageState({ path: 'auth-state.json' });
   ```

2. **Get Clerk Auth Token**
   ```typescript
   const token = await page.evaluate(async () => {
     return await window.Clerk.session.getToken({
       template: 'private-beach-manager'
     });
   });
   ```

3. **Create Beach via API**
   ```typescript
   const response = await fetch('http://localhost:8080/private-beaches', {
     method: 'POST',
     headers: {
       'Authorization': `Bearer ${token}`,
       'Content-Type': 'application/json',
     },
     body: JSON.stringify({
       name: 'Test Beach',
       slug: 'test-beach-' + Date.now(),
     }),
   });
   const beach = await response.json();
   const beachId = beach.id;
   ```

4. **Attach Session to Beach**
   ```typescript
   await fetch(`http://localhost:8080/private-beaches/${beachId}/sessions/attach-by-code`, {
     method: 'POST',
     headers: {
       'Authorization': `Bearer ${token}`,
       'Content-Type': 'application/json',
     },
     body: JSON.stringify({
       session_id: process.env.BEACH_TEST_SESSION_ID,
       code: process.env.BEACH_TEST_PASSCODE,
     }),
   });
   ```

5. **Run Test**
   ```bash
   export BEACH_TEST_SESSION_ID="<session-id>"
   export BEACH_TEST_PASSCODE="<passcode>"
   export BEACH_ID="<beach-id-from-step-3>"

   cd apps/private-beach
   npx playwright test tile-resize-hud.spec.ts --workers=1
   ```

### Method 2: Manual Beach Creation

If automated beach creation fails:

1. Navigate to http://localhost:3000
2. Sign in as testuser
3. Click "New Beach"
4. Enter session ID and passcode
5. Note the beach ID from URL
6. Run test with BEACH_ID set

## Test Verification

### Success Criteria

The test should:

1. ✅ Connect to Private Beach session
2. ✅ Find tile by session ID
3. ✅ Wait for terminal to connect and show "Connected" badge
4. ✅ Capture initial terminal state (24 rows)
5. ✅ Resize tile to ~70 rows via drag handle
6. ✅ Wait 5 seconds for PTY backfill
7. ✅ Capture terminal state after resize
8. ✅ Analyze for duplicate HUD content
9. ✅ Assert 0 duplicates found
10. ✅ Take screenshots for verification

### Expected Output

```
Initial state: { viewportRows: 24, gridRows: 24, rowsLoaded: 24 }
After resize: { viewportRows: 70, gridRows: 70, rowsLoaded: 70 }
✓ No duplicate HUD content detected
✓ Test passed
```

## Troubleshooting

### Common Issues

#### 1. Port Conflicts

**Symptom:** "Address already in use (os error 48)"

**Solution:**
```bash
# Find process using port 8080
lsof -i :8080

# Kill if needed
kill -9 <PID>

# Or use different ports (update .env.local accordingly)
```

#### 2. beach-road Not on Correct Port

**Symptom:** beach-manager fails to start because beach-road is on 8080

**Solution:**
- Implement PORT configuration in beach-road
- Or run beach-road in separate environment with modified hardcoded port

#### 3. PostgreSQL Not Ready

**Symptom:** beach-manager crashes with "connection refused"

**Solution:**
```bash
# Wait for PostgreSQL to be fully ready
timeout 30 bash -c 'until docker exec beach-postgres pg_isready -U postgres; do sleep 1; done'
```

#### 4. Database Not Migrated

**Symptom:** beach-manager returns 500 errors

**Solution:**
```bash
export DATABASE_URL="postgres://postgres:postgres@localhost:5432/beach_manager"
cd apps/beach-manager
sqlx migrate run --source migrations
```

#### 5. Clerk Authentication Fails

**Symptom:** "Password compromised" or authentication errors

**Solution:**
- Use strong password: `beach r0cks!`
- Or use email code authentication method
- Ensure Clerk dev keys are in .env.local

## File Locations

### Test Files
- `apps/private-beach/tests/e2e/tile-resize-hud.spec.ts` - Main test
- `apps/private-beach/tests/e2e/helpers/terminal-capture.ts` - Terminal state capture
- `apps/private-beach/tests/e2e/helpers/tile-manipulation.ts` - Tile resizing
- `apps/private-beach/tests/e2e/fixtures/beach-session.ts` - Session credentials

### Configuration Files
- `apps/private-beach/.env.local` - Environment variables
- `apps/private-beach/playwright.config.ts` - Playwright config

### Documentation
- `docs/pty-resizing-issues/test-execution-and-diagnostics.md` - Test execution guide
- `docs/pty-resizing-issues/private-beach-duplicate-hud.md` - Bug description

## Scripts to Create

### `scripts/start-private-beach-test-env.sh`

Full automation script that:
1. Starts all services in correct order
2. Waits for health checks
3. Creates Beach session
4. Outputs credentials
5. Waits for user to create beach manually OR automates beach creation
6. Runs test
7. Cleans up

### `scripts/stop-private-beach-test-env.sh`

Cleanup script that:
1. Stops all background processes
2. Stops Docker containers
3. Cleans up temporary files

## Next Steps for Implementation

1. **Fix beach-road Port Configuration**
   - Add PORT/BEACH_ROAD_PORT environment variable support
   - Update default to 4132 to match .env.local

2. **Create Playwright Auth Helper**
   - File: `apps/private-beach/tests/e2e/helpers/clerk-auth.ts`
   - Functions: `signIn()`, `getToken()`, `saveAuthState()`

3. **Create Beach Setup Helper**
   - File: `apps/private-beach/tests/e2e/helpers/beach-setup.ts`
   - Functions: `createBeach()`, `attachSession()`, `getBeachId()`

4. **Update Test to Use Helpers**
   - Remove manual beach creation requirement
   - Fully automate from fresh state

5. **Create Test Orchestration Script**
   - Single command to run everything
   - Handles all infrastructure setup
   - Runs test and reports results

## Alternative: Use Standalone Test

Since the standalone beach-surfer test already passes and uses the same code path, it's valid to consider that test as verification of the fix. The Private Beach test adds:

- React-grid-layout tile drag-resize interaction
- Private Beach UI integration
- Multi-service infrastructure verification

But the core bug fix (cache.ts setGridSize) is identical in both scenarios.

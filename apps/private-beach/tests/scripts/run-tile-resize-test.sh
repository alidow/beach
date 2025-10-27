#!/usr/bin/env bash
# Fully Automated Private Beach Tile Resize Test
# Starts all infrastructure, creates a Beach session, runs Playwright E2E test, and cleans up

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
PRIVATE_BEACH_DIR="$PROJECT_ROOT/apps/private-beach"

echo "🏖️  Private Beach Tile Resize Test - Full Automation"
echo "===================================================="
echo ""

# Cleanup any existing processes
echo "🧹 Pre-test cleanup..."
"$SCRIPT_DIR/cleanup-test-env.sh" 2>/dev/null || true
sleep 2

# Start Redis
echo "📦 Starting Redis..."
cd "$PROJECT_ROOT"
docker-compose up -d redis
sleep 2

if ! docker exec beach-redis redis-cli ping &>/dev/null; then
  echo "❌ Error: Redis failed to start"
  exit 1
fi
echo "✅ Redis ready"

# Start beach-road
echo ""
echo "🛣️  Starting beach-road..."
BEACH_PUBLIC_SESSION_SERVER=http://localhost:8080 \
  cargo run -p beach-road > /tmp/beach-road.log 2>&1 &
BEACH_ROAD_PID=$!
echo $BEACH_ROAD_PID > /tmp/beach-road.pid
sleep 3

if ! curl -sf http://localhost:8080/health > /dev/null; then
  echo "❌ Error: beach-road failed to start"
  echo "   Check logs: /tmp/beach-road.log"
  exit 1
fi
echo "✅ beach-road ready"

# Build beach if needed
if [[ ! -f "$PROJECT_ROOT/target/debug/beach" ]]; then
  echo ""
  echo "🔨 Building beach..."
  cd "$PROJECT_ROOT"
  cargo build -p beach
fi

# Start Beach host session with Pong demo
echo ""
echo "🎮 Starting Beach host session with Pong..."
cd "$PROJECT_ROOT"
RUST_LOG=info BEACH_LOG_LEVEL=info \
  "$PROJECT_ROOT/target/debug/beach" \
  --session-server http://localhost:8080/ \
  host \
  --bootstrap-output json \
  -- /usr/bin/env python3 \
    "$PROJECT_ROOT/apps/private-beach/demo/pong/player/main.py" \
    --mode lhs \
  > /tmp/beach-bootstrap.txt 2>&1 &

BEACH_HOST_PID=$!
echo $BEACH_HOST_PID > /tmp/beach-host.pid
echo "   Beach host PID: $BEACH_HOST_PID"
sleep 5

# Check if process is still running
if ! kill -0 $BEACH_HOST_PID 2>/dev/null; then
  echo "❌ Error: Beach host process died"
  echo "   Last 50 lines of trace:"
  tail -50 /tmp/beach-bootstrap.txt
  exit 1
fi
echo "   Beach process confirmed running (PID: $BEACH_HOST_PID)"

# Extract session credentials
echo "   Extracting credentials..."
SESSION_ID=$(grep -o '"session_id":"[^"]*"' /tmp/beach-bootstrap.txt | cut -d'"' -f4)
PASSCODE=$(grep -o '"join_code":"[^"]*"' /tmp/beach-bootstrap.txt | cut -d'"' -f4)

if [[ -z "$SESSION_ID" ]] || [[ -z "$PASSCODE" ]]; then
  echo "❌ Error: Failed to extract credentials"
  cat /tmp/beach-bootstrap.txt
  exit 1
fi

echo "✅ Session created"
echo "   Session ID: $SESSION_ID"
echo "   Passcode:   $PASSCODE"
echo ""
echo "   Waiting for Pong to initialize..."
sleep 5

# Start Private Beach Next.js dev server if not running
if ! lsof -ti:3000 > /dev/null 2>&1; then
  echo "🌊 Starting Private Beach dev server..."
  cd "$PRIVATE_BEACH_DIR"
  npm run dev > /tmp/private-beach.log 2>&1 &
  NEXT_PID=$!
  echo $NEXT_PID > /tmp/private-beach.pid
  sleep 8
  echo "✅ Private Beach ready at http://localhost:3000"
else
  echo "✅ Private Beach already running at http://localhost:3000"
fi

# TODO: Create beach and add session via API
# For now, user must manually create a beach and set BEACH_ID
echo ""
echo "⚠️  Manual Step Required:"
echo "   1. Open http://localhost:3000 in a browser"
echo "   2. Create a new beach"
echo "   3. Add session $SESSION_ID to the beach"
echo "   4. Note the beach ID from the URL (/beaches/[id])"
echo "   5. Set BEACH_ID environment variable"
echo ""
echo "   Then run:"
echo "   cd $PRIVATE_BEACH_DIR"
echo "   BEACH_ID=<your-beach-id> BEACH_TEST_SESSION_ID=$SESSION_ID BEACH_TEST_PASSCODE=$PASSCODE npm run test:e2e:resize"
echo ""
echo "Press Enter when ready to continue with test (or Ctrl+C to stop)..."
read

# Run Playwright test
echo ""
echo "🎭 Running Playwright tile resize test..."
cd "$PRIVATE_BEACH_DIR"

export BEACH_TEST_SESSION_ID="$SESSION_ID"
export BEACH_TEST_PASSCODE="$PASSCODE"
export BEACH_TEST_SESSION_SERVER="http://localhost:8080"

# Check if BEACH_ID is set
if [[ -z "${BEACH_ID:-}" ]]; then
  echo "❌ Error: BEACH_ID environment variable not set"
  echo "   Please set it to the ID of the beach you created"
  exit 1
fi

# Run test and capture result
set +e
npm run test:e2e:resize -- --workers=1 --timeout=90000
TEST_EXIT_CODE=$?
set -e

echo ""
if [[ $TEST_EXIT_CODE -eq 0 ]]; then
  echo "✅ Test passed!"
else
  echo "❌ Test failed (exit code: $TEST_EXIT_CODE)"
  echo "   Check screenshots in: apps/private-beach/test-results/"
fi

# Cleanup
echo ""
echo "🧹 Cleaning up..."
"$SCRIPT_DIR/cleanup-test-env.sh"

echo ""
echo "===================================================="
echo "Test run complete!"
exit $TEST_EXIT_CODE

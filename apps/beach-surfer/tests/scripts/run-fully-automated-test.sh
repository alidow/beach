#!/usr/bin/env bash
# Fully automated PTY resize test runner
# Starts infrastructure, creates session, runs Playwright test, and cleans up

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"

echo "🏖️  Fully Automated Beach PTY Resize Test"
echo "=========================================="
echo ""

# Cleanup any existing processes
echo "🧹 Pre-test cleanup..."
"$SCRIPT_DIR/cleanup-resize-test.sh" 2>/dev/null || true
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
if [[ ! -f "target/debug/beach" ]]; then
  echo ""
  echo "🔨 Building beach..."
  cargo build -p beach
fi

# Start Beach host session with Pong demo
echo ""
echo "🎮 Starting Beach host session with Pong..."
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

# Start beach-surfer dev server if not running
if ! lsof -ti:5173 > /dev/null 2>&1; then
  echo "🌊 Starting beach-surfer dev server..."
  cd "$PROJECT_ROOT/apps/beach-surfer"
  npm run dev > /tmp/beach-surfer.log 2>&1 &
  VITE_PID=$!
  echo $VITE_PID > /tmp/beach-surfer.pid
  sleep 5
  echo "✅ beach-surfer ready"
else
  echo "✅ beach-surfer already running"
fi

# Run Playwright test
echo ""
echo "🎭 Running Playwright resize test..."
cd "$PROJECT_ROOT/apps/beach-surfer"

export BEACH_TEST_SESSION_ID="$SESSION_ID"
export BEACH_TEST_PASSCODE="$PASSCODE"
export BEACH_TEST_SESSION_SERVER="http://localhost:8080"

# Run test and capture result
set +e
npm run test:e2e:resize -- --workers=1 --timeout=60000
TEST_EXIT_CODE=$?
set -e

echo ""
if [[ $TEST_EXIT_CODE -eq 0 ]]; then
  echo "✅ Test passed!"
else
  echo "❌ Test failed (exit code: $TEST_EXIT_CODE)"
  echo "   Check screenshots in: apps/beach-surfer/test-results/"
fi

# Cleanup
echo ""
echo "🧹 Cleaning up..."
"$SCRIPT_DIR/cleanup-resize-test.sh"

echo ""
echo "=========================================="
echo "Test run complete!"
exit $TEST_EXIT_CODE

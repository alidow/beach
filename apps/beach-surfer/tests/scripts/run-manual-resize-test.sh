#!/usr/bin/env bash
# Semi-automated PTY resize test runner
# Starts all infrastructure, then guides user through manual testing

set -euo pipefail

echo "🏖️  Beach PTY Resize Test Runner"
echo "================================"
echo ""

# Check prerequisites
if ! command -v docker-compose &> /dev/null; then
  echo "❌ Error: docker-compose not found"
  exit 1
fi

if ! command -v cargo &> /dev/null; then
  echo "❌ Error: cargo not found"
  exit 1
fi

if ! command -v npm &> /dev/null; then
  echo "❌ Error: npm not found"
  exit 1
fi

# Cleanup any existing processes
echo "🧹 Cleaning up existing processes..."
pkill -f "beach-road" 2>/dev/null || true
pkill -f "beach.*host" 2>/dev/null || true
pkill -f "python.*pong" 2>/dev/null || true
sleep 1

# Start Redis
echo ""
echo "📦 Starting Redis..."
docker-compose up -d redis
sleep 2

# Verify Redis
if ! docker exec beach-redis redis-cli ping &>/dev/null; then
  echo "❌ Error: Redis failed to start"
  exit 1
fi
echo "✅ Redis ready"

# Start beach-road
echo ""
echo "🛣️  Starting beach-road..."
cargo run -p beach-road > /tmp/beach-road.log 2>&1 &
BEACH_ROAD_PID=$!
echo $BEACH_ROAD_PID > /tmp/beach-road.pid
sleep 3

# Verify beach-road
if ! curl -sf http://localhost:8080/health > /dev/null; then
  echo "❌ Error: beach-road failed to start"
  echo "   Check logs: /tmp/beach-road.log"
  exit 1
fi
echo "✅ beach-road listening on http://localhost:8080"

# Build beach if needed
if [[ ! -f "target/debug/beach" ]]; then
  echo ""
  echo "🔨 Building beach..."
  cargo build -p beach
fi

# Start Beach host session with Pong
echo ""
echo "🎮 Starting Beach host session with Pong..."
env BEACH_LOG_LEVEL=warn \
  ./target/debug/beach \
  --session-server http://localhost:8080/ \
  host \
  --bootstrap-output json \
  --wait \
  -- /usr/bin/env python3 \
    "$PWD/apps/private-beach/demo/pong/player/main.py" \
    --mode lhs \
  > /tmp/beach-bootstrap.txt 2>&1 &

BEACH_HOST_PID=$!
echo $BEACH_HOST_PID > /tmp/beach-host.pid
echo "   Beach host PID: $BEACH_HOST_PID"

# Wait for bootstrap JSON
echo "   Waiting for bootstrap output..."
sleep 5

# Check if process is still running
if ! kill -0 $BEACH_HOST_PID 2>/dev/null; then
  echo "❌ Error: Beach host process died"
  echo "   Check output: /tmp/beach-bootstrap.txt"
  cat /tmp/beach-bootstrap.txt
  exit 1
fi

# Extract session credentials
if [[ ! -f "/tmp/beach-bootstrap.txt" ]]; then
  echo "❌ Error: Bootstrap file not found"
  exit 1
fi

BOOTSTRAP_JSON=$(head -1 /tmp/beach-bootstrap.txt)
SESSION_ID=$(echo "$BOOTSTRAP_JSON" | grep -o '"session_id":"[^"]*"' | cut -d'"' -f4)
PASSCODE=$(echo "$BOOTSTRAP_JSON" | grep -o '"join_code":"[^"]*"' | cut -d'"' -f4)

if [[ -z "$SESSION_ID" ]] || [[ -z "$PASSCODE" ]]; then
  echo "❌ Error: Failed to extract session credentials"
  echo "   Bootstrap output:"
  cat /tmp/beach-bootstrap.txt
  exit 1
fi

echo "✅ Beach session started!"
echo ""
echo "   Session ID: $SESSION_ID"
echo "   Passcode:   $PASSCODE"
echo ""
echo "   Waiting for Pong to initialize..."
sleep 3

# Start beach-surfer dev server (if not already running)
if ! lsof -ti:5173 > /dev/null 2>&1; then
  echo "🌊 Starting beach-surfer dev server..."
  cd apps/beach-surfer
  npm run dev > /tmp/beach-surfer.log 2>&1 &
  VITE_PID=$!
  echo $VITE_PID > /tmp/beach-surfer.pid
  cd ../..
  sleep 5
  echo "✅ beach-surfer ready at http://localhost:5173"
else
  echo "✅ beach-surfer already running at http://localhost:5173"
fi

# Display test instructions
echo ""
echo "╔════════════════════════════════════════════════════════════╗"
echo "║                  MANUAL TEST INSTRUCTIONS                   ║"
echo "╚════════════════════════════════════════════════════════════╝"
echo ""
echo "1. Open beach-surfer in your browser:"
echo "   → http://localhost:5173"
echo ""
echo "2. Enter session credentials:"
echo "   Session ID: $SESSION_ID"
echo "   Passcode:   $PASSCODE"
echo ""
echo "3. Click 'Connect' and wait for terminal to load"
echo ""
echo "4. You should see the Pong HUD with commands:"
echo "   - Ready."
echo "   - Commands: m <delta> | b <y> <dx> <dy> | quit"
echo "   - Mode: LHS — Paddle X=3 (positive delta moves up)"
echo "   - >"
echo ""
echo "5. RESIZE TEST:"
echo "   a) Note current viewport size"
echo "   b) Drag browser window corner to make it TALLER"
echo "   c) Observe the newly exposed rows at the TOP"
echo ""
echo "6. EXPECTED (Fix Working):"
echo "   ✅ New rows are BLANK"
echo "   ✅ No duplicate HUD content"
echo ""
echo "7. BUG (Fix Not Working):"
echo "   ❌ New rows show DUPLICATE HUD lines"
echo "   ❌ \"Unknown command\", \"Commands\", \"Mode\", \">\" repeated"
echo ""
echo "8. DEBUG (optional):"
echo "   In browser console:"
echo "   → window.__BEACH_TRACE = true"
echo "   → window.__BEACH_TRACE_DUMP_ROWS(20)"
echo ""
echo "Press Ctrl+C when done testing to cleanup..."
echo ""

# Save session info for reference
cat > /tmp/beach-test-session.txt <<EOF
Beach PTY Resize Test Session
=============================
Started: $(date)

Session ID: $SESSION_ID
Passcode:   $PASSCODE
Server:     http://localhost:8080

Process IDs:
- beach-road: $BEACH_ROAD_PID
- beach host: $BEACH_HOST_PID
- vite: $(cat /tmp/beach-surfer.pid 2>/dev/null || echo "N/A")

Logs:
- beach-road: /tmp/beach-road.log
- beach host: /tmp/beach-bootstrap.txt
- vite: /tmp/beach-surfer.log

Cleanup command:
./apps/beach-surfer/tests/scripts/cleanup-resize-test.sh
EOF

# Wait for user interrupt
trap "echo ''; echo '🧹 Cleaning up...'; ./apps/beach-surfer/tests/scripts/cleanup-resize-test.sh; exit 0" INT

# Keep script running
while true; do
  sleep 1
done

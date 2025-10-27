#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "🏖️  Starting Private Beach Test Infrastructure"
echo "=============================================="

# Clean up any existing processes first
echo "Cleaning up any existing processes..."
for pidfile in /tmp/beach-road.pid /tmp/beach-manager.pid /tmp/beach-host.pid /tmp/private-beach.pid; do
  if [ -f "$pidfile" ]; then
    kill $(cat "$pidfile") 2>/dev/null || true
    rm "$pidfile"
  fi
done

# 1. Start Docker services
echo ""
echo "📦 Starting Docker services (postgres, redis)..."
cd "$PROJECT_ROOT"
docker-compose up -d postgres redis

# Wait for databases to be ready
echo "⏳ Waiting for databases to be ready..."
timeout 30 bash -c 'until docker exec beach-postgres pg_isready -U postgres 2>/dev/null; do sleep 1; done' || {
  echo "❌ PostgreSQL failed to start"
  exit 1
}
timeout 30 bash -c 'until docker exec beach-redis redis-cli ping 2>/dev/null | grep -q PONG; do sleep 1; done' || {
  echo "❌ Redis failed to start"
  exit 1
}
echo "✅ Databases ready"

# 2. Run database migrations
echo ""
echo "🗄️  Running database migrations..."
export DATABASE_URL="postgres://postgres:postgres@localhost:5432/beach_manager"
cd "$PROJECT_ROOT/apps/beach-manager"
if command -v sqlx &> /dev/null; then
  sqlx migrate run --source migrations || {
    echo "❌ Migrations failed"
    exit 1
  }
else
  echo "⚠️  sqlx-cli not found, skipping migrations (assuming already run)"
fi
cd "$PROJECT_ROOT"
echo "✅ Migrations complete"

# 3. Start beach-road on port 4132
echo ""
echo "🌊 Starting beach-road on port 4132..."
export BEACH_ROAD_PORT=4132
export REDIS_URL="redis://localhost:6379"
export RUST_LOG=warn
cargo build -p beach-road --release 2>&1 | grep -E "(Compiling|Finished)" || true
cargo run -p beach-road --release > /tmp/beach-road.log 2>&1 &
BEACH_ROAD_PID=$!
echo $BEACH_ROAD_PID > /tmp/beach-road.pid
echo "beach-road PID: $BEACH_ROAD_PID"

# Wait for beach-road to start
echo "⏳ Waiting for beach-road to be ready..."
for i in {1..10}; do
  sleep 1
  if curl -sf http://localhost:4132/health > /dev/null 2>&1; then
    echo "✅ beach-road ready"
    break
  fi
  if [ $i -eq 10 ]; then
    echo "❌ beach-road failed to start"
    cat /tmp/beach-road.log
    exit 1
  fi
done

# 4. Start beach-manager on port 8080
echo ""
echo "🏖️  Starting beach-manager on port 8080..."
export DATABASE_URL="postgres://postgres:postgres@localhost:5432/beach_manager"
export PORT=8080
export BEACH_ROAD_URL="http://localhost:4132"
cargo build -p beach-manager --release 2>&1 | grep -E "(Compiling|Finished)" || true
cargo run -p beach-manager --release > /tmp/beach-manager.log 2>&1 &
BEACH_MANAGER_PID=$!
echo $BEACH_MANAGER_PID > /tmp/beach-manager.pid
echo "beach-manager PID: $BEACH_MANAGER_PID"

# Wait for beach-manager to start
echo "⏳ Waiting for beach-manager to be ready..."
for i in {1..10}; do
  sleep 1
  if curl -sf http://localhost:8080/healthz > /dev/null 2>&1; then
    echo "✅ beach-manager ready"
    break
  fi
  if [ $i -eq 10 ]; then
    echo "❌ beach-manager failed to start"
    cat /tmp/beach-manager.log
    exit 1
  fi
done

# 5. Start Beach host session with Pong demo
echo ""
echo "🎮 Starting Beach host session with Pong demo..."
cargo build -p beach 2>&1 | grep -E "(Compiling|Finished)" || true

# Create log directory
mkdir -p "$HOME/beach-debug"

# Start host in background and capture output
# Use script to capture bootstrap, similar to manual testing setup
export BEACH_LOG_LEVEL=trace
export BEACH_LOG_FILTER="transport::webrtc=trace,server::grid=trace,sync::incoming=trace,host::stdin=trace"

script -q "$HOME/beach-debug/bootstrap.raw" \
  bash -c "env \
    BEACH_LOG_LEVEL=trace \
    BEACH_LOG_FILTER='transport::webrtc=trace,server::grid=trace,sync::incoming=trace,host::stdin=trace' \
    $PROJECT_ROOT/target/debug/beach \
      --log-level trace \
      --log-file /tmp/beach-host.log \
      --session-server http://localhost:4132/ \
      host \
      --bootstrap-output json \
      --wait \
      -- /usr/bin/env python3 $PROJECT_ROOT/apps/private-beach/demo/pong/player/main.py --mode lhs" \
  > /tmp/beach-bootstrap.txt 2>&1 &
BEACH_HOST_PID=$!
echo $BEACH_HOST_PID > /tmp/beach-host.pid
echo "beach host PID: $BEACH_HOST_PID"

# Wait for bootstrap output
echo "⏳ Waiting for session bootstrap..."
for i in {1..15}; do
  sleep 1
  if grep -q "session_id" /tmp/beach-bootstrap.txt 2>/dev/null; then
    break
  fi
  if [ $i -eq 15 ]; then
    echo "❌ Beach host failed to start"
    cat /tmp/beach-bootstrap.txt
    exit 1
  fi
done

# Extract credentials from bootstrap output
echo "📋 Extracting session credentials..."
SESSION_ID=$(grep -o '"session_id":"[^"]*"' /tmp/beach-bootstrap.txt | head -1 | cut -d'"' -f4)
PASSCODE=$(grep -o '"join_code":"[^"]*"' /tmp/beach-bootstrap.txt | head -1 | cut -d'"' -f4)

if [ -z "$SESSION_ID" ] || [ -z "$PASSCODE" ]; then
  echo "❌ Failed to extract session credentials"
  cat /tmp/beach-bootstrap.txt
  exit 1
fi

echo "✅ Session created:"
echo "   Session ID: $SESSION_ID"
echo "   Passcode: $PASSCODE"

# Export for tests
export BEACH_TEST_SESSION_ID="$SESSION_ID"
export BEACH_TEST_PASSCODE="$PASSCODE"
export BEACH_TEST_SESSION_SERVER="http://localhost:4132"
export BEACH_TEST_MANAGER_URL="http://localhost:8080"

# Write credentials to file for test scripts to source
cat > /tmp/beach-test-env.sh <<EOF
export BEACH_TEST_SESSION_ID="$SESSION_ID"
export BEACH_TEST_PASSCODE="$PASSCODE"
export BEACH_TEST_SESSION_SERVER="http://localhost:4132"
export BEACH_TEST_MANAGER_URL="http://localhost:8080"
EOF

# 6. Start Private Beach dev server (if not running)
echo ""
echo "🌐 Starting Private Beach dev server..."
if lsof -ti:3000 > /dev/null 2>&1; then
  echo "⚠️  Port 3000 already in use, assuming dev server is running"
else
  cd "$PROJECT_ROOT/apps/private-beach"
  npm run dev > /tmp/private-beach.log 2>&1 &
  NEXT_PID=$!
  echo $NEXT_PID > /tmp/private-beach.pid
  echo "Private Beach PID: $NEXT_PID"
  cd "$PROJECT_ROOT"

  # Wait for dev server
  echo "⏳ Waiting for Private Beach to be ready..."
  for i in {1..30}; do
    sleep 1
    if curl -sf http://localhost:3000 > /dev/null 2>&1; then
      echo "✅ Private Beach ready"
      break
    fi
    if [ $i -eq 30 ]; then
      echo "❌ Private Beach failed to start"
      cat /tmp/private-beach.log
      exit 1
    fi
  done
fi

echo ""
echo "=============================================="
echo "✅ Infrastructure ready!"
echo ""
echo "Session credentials:"
echo "  Session ID: $SESSION_ID"
echo "  Passcode: $PASSCODE"
echo ""
echo "Services:"
echo "  beach-road:      http://localhost:4132"
echo "  beach-manager:   http://localhost:8080"
echo "  Private Beach:   http://localhost:3000"
echo ""
echo "Run tests with: cd apps/private-beach && npm run test:e2e:tile-resize"
echo "Or source env: source /tmp/beach-test-env.sh"
echo ""

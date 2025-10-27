#!/usr/bin/env bash
# Cleanup script for Private Beach tile resize tests
# Stops all test-related processes and cleans up temporary files

set -eo pipefail

echo "🧹 Cleaning up Private Beach tile resize test environment..."

# Stop Private Beach Next.js dev server
if [[ -f /tmp/private-beach.pid ]]; then
  PID=$(cat /tmp/private-beach.pid)
  if kill -0 "$PID" 2>/dev/null; then
    echo "   Stopping Private Beach dev server (PID: $PID)..."
    kill "$PID" 2>/dev/null || true
    sleep 1
    kill -9 "$PID" 2>/dev/null || true
  fi
  rm -f /tmp/private-beach.pid
fi

# Alternative: Kill by port
if lsof -ti:3000 > /dev/null 2>&1; then
  echo "   Stopping process on port 3000..."
  lsof -ti:3000 | xargs kill -9 2>/dev/null || true
fi

# Stop Beach host session
if [[ -f /tmp/beach-host.pid ]]; then
  PID=$(cat /tmp/beach-host.pid)
  if kill -0 "$PID" 2>/dev/null; then
    echo "   Stopping Beach host (PID: $PID)..."
    kill "$PID" 2>/dev/null || true
    sleep 1
    kill -9 "$PID" 2>/dev/null || true
  fi
  rm -f /tmp/beach-host.pid
fi

# Stop beach-road
if [[ -f /tmp/beach-road.pid ]]; then
  PID=$(cat /tmp/beach-road.pid)
  if kill -0 "$PID" 2>/dev/null; then
    echo "   Stopping beach-road (PID: $PID)..."
    kill "$PID" 2>/dev/null || true
    sleep 1
    kill -9 "$PID" 2>/dev/null || true
  fi
  rm -f /tmp/beach-road.pid
fi

# Alternative: Kill by port
if lsof -ti:8080 > /dev/null 2>&1; then
  echo "   Stopping process on port 8080..."
  lsof -ti:8080 | xargs kill -9 2>/dev/null || true
fi

# Stop Docker containers
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
cd "$PROJECT_ROOT"

if docker ps -q --filter "name=beach-redis" | grep -q .; then
  echo "   Stopping Docker containers..."
  docker-compose down 2>/dev/null || true
fi

# Remove temp files
echo "   Removing temp files..."
rm -f /tmp/beach-bootstrap.txt
rm -f /tmp/beach-road.log
rm -f /tmp/private-beach.log

echo "✅ Cleanup complete"

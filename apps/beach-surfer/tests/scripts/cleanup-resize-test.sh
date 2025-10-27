#!/usr/bin/env bash
# Cleanup all processes from resize test
set -euo pipefail

echo "🧹 Cleaning up Beach resize test..."

# Kill beach-road
if [[ -f "/tmp/beach-road.pid" ]]; then
  PID=$(cat /tmp/beach-road.pid)
  if kill -0 $PID 2>/dev/null; then
    echo "   Stopping beach-road (PID: $PID)..."
    kill $PID 2>/dev/null || true
  fi
  rm -f /tmp/beach-road.pid
fi

# Kill beach host
if [[ -f "/tmp/beach-host.pid" ]]; then
  PID=$(cat /tmp/beach-host.pid)
  if kill -0 $PID 2>/dev/null; then
    echo "   Stopping beach host (PID: $PID)..."
    kill $PID 2>/dev/null || true
  fi
  rm -f /tmp/beach-host.pid
fi

# Kill vite
if [[ -f "/tmp/beach-surfer.pid" ]]; then
  PID=$(cat /tmp/beach-surfer.pid)
  if kill -0 $PID 2>/dev/null; then
    echo "   Stopping vite (PID: $PID)..."
    kill $PID 2>/dev/null || true
  fi
  rm -f /tmp/beach-surfer.pid
fi

# Kill any orphaned processes
pkill -f "beach-road" 2>/dev/null || true
pkill -f "beach.*host" 2>/dev/null || true
pkill -f "python.*pong" 2>/dev/null || true
pkill -f "script.*bootstrap" 2>/dev/null || true

# Stop Docker containers
echo "   Stopping Docker containers..."
docker-compose down 2>/dev/null || true

# Clean up log files
echo "   Removing temp files..."
rm -f /tmp/beach-road.log
rm -f /tmp/beach-bootstrap.txt
rm -f /tmp/beach-surfer.log
rm -f /tmp/beach-test-session.txt

echo "✅ Cleanup complete"

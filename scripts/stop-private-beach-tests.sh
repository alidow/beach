#!/usr/bin/env bash
set -euo pipefail

echo "🧹 Stopping Private Beach Test Infrastructure"
echo "=============================================="

# Kill processes
echo "Stopping services..."

for pidfile in /tmp/beach-road.pid /tmp/beach-manager.pid /tmp/beach-host.pid /tmp/private-beach.pid; do
  if [ -f "$pidfile" ]; then
    PID=$(cat "$pidfile")
    SERVICE=$(basename "$pidfile" .pid)
    if kill -0 "$PID" 2>/dev/null; then
      echo "  Stopping $SERVICE (PID: $PID)..."
      kill "$PID" 2>/dev/null || true
      # Wait a bit for graceful shutdown
      sleep 1
      # Force kill if still running
      kill -9 "$PID" 2>/dev/null || true
    fi
    rm "$pidfile"
  fi
done

# Stop Docker containers
echo "Stopping Docker containers..."
docker-compose stop postgres redis 2>/dev/null || true

# Clean up temp files
echo "Cleaning up temp files..."
rm -f /tmp/beach-bootstrap.txt
rm -f /tmp/beach-test-env.sh
rm -f /tmp/beach-road.log
rm -f /tmp/beach-manager.log
rm -f /tmp/beach-host.log
rm -f /tmp/private-beach.log

echo ""
echo "✅ Cleanup complete"
echo ""

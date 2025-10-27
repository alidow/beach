#!/usr/bin/env bash
# Cleanup test Beach session and processes
# Usage: cleanup-test-session.sh [--keep-logs]

set -euo pipefail

KEEP_LOGS=false
if [[ "${1:-}" == "--keep-logs" ]]; then
  KEEP_LOGS=true
fi

LOG_DIR="${BEACH_DEBUG_DIR:-$HOME/beach-debug}"
PID_FILE="$LOG_DIR/beach-host.pid"

echo "🧹 Cleaning up Beach test session..."

# Kill host session if PID file exists
if [[ -f "$PID_FILE" ]]; then
  HOST_PID=$(cat "$PID_FILE")
  if kill -0 "$HOST_PID" 2>/dev/null; then
    echo "   Stopping host session (PID: $HOST_PID)..."
    kill "$HOST_PID" 2>/dev/null || true
    sleep 1

    # Force kill if still running
    if kill -0 "$HOST_PID" 2>/dev/null; then
      echo "   Force killing host session..."
      kill -9 "$HOST_PID" 2>/dev/null || true
    fi
  fi
  rm -f "$PID_FILE"
fi

# Kill any orphaned beach processes
BEACH_PIDS=$(pgrep -f "beach.*host.*pong" || true)
if [[ -n "$BEACH_PIDS" ]]; then
  echo "   Killing orphaned beach processes: $BEACH_PIDS"
  echo "$BEACH_PIDS" | xargs kill 2>/dev/null || true
fi

# Kill any orphaned python/pong processes
PONG_PIDS=$(pgrep -f "python.*pong.*main.py" || true)
if [[ -n "$PONG_PIDS" ]]; then
  echo "   Killing orphaned pong processes: $PONG_PIDS"
  echo "$PONG_PIDS" | xargs kill 2>/dev/null || true
fi

# Kill any orphaned script processes
SCRIPT_PIDS=$(pgrep -f "script.*bootstrap.raw" || true)
if [[ -n "$SCRIPT_PIDS" ]]; then
  echo "   Killing orphaned script processes: $SCRIPT_PIDS"
  echo "$SCRIPT_PIDS" | xargs kill 2>/dev/null || true
fi

# Clean up session files
if [[ "$KEEP_LOGS" == "false" ]]; then
  echo "   Removing session files from $LOG_DIR..."
  rm -f "$LOG_DIR/bootstrap.raw"
  rm -f "$LOG_DIR/session-creds.env"
  rm -f "$LOG_DIR/session-output.log"
else
  echo "   Keeping logs in $LOG_DIR (use --keep-logs to preserve)"
fi

echo "✅ Cleanup complete"

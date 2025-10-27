#!/usr/bin/env bash
# Launch a Beach host session with Pong demo for testing
# This script captures the bootstrap output and keeps the session running

set -euo pipefail

# Configuration
LOG_DIR="${BEACH_DEBUG_DIR:-$HOME/beach-debug}"
BOOTSTRAP_FILE="$LOG_DIR/bootstrap.raw"
PID_FILE="$LOG_DIR/beach-host.pid"
SESSION_CREDS_FILE="$LOG_DIR/session-creds.env"
BEACH_LOG_FILE="${BEACH_LOG_FILE:-/tmp/beach-host.log}"

# Ensure log directory exists
mkdir -p "$LOG_DIR"

# Clean up old bootstrap and PID files
rm -f "$BOOTSTRAP_FILE" "$PID_FILE" "$SESSION_CREDS_FILE"

echo "🏖️  Starting Beach host session with Pong demo..."
echo "   Bootstrap output: $BOOTSTRAP_FILE"
echo "   Session log: $BEACH_LOG_FILE"

# Build beach if needed
if [[ ! -f "target/debug/beach" ]]; then
  echo "   Building beach..."
  cargo build -p beach
fi

# Start the host session in the background using script to capture PTY output
# We use a subshell to properly background the process
(
  script -q "$BOOTSTRAP_FILE" bash -lc "
    export BEACH_LOG_LEVEL=trace
    export BEACH_LOG_FILTER='transport::webrtc=trace,server::grid=trace,sync::incoming=trace,host::stdin=trace'

    exec $PWD/target/debug/beach \
      --log-level trace \
      --log-file '$BEACH_LOG_FILE' \
      --session-server http://localhost:8080/ \
      host \
      --bootstrap-output json \
      --wait \
      -- /usr/bin/env python3 '$PWD/apps/private-beach/demo/pong/player/main.py' --mode lhs
  "
) > "$LOG_DIR/session-output.log" 2>&1 &

HOST_PID=$!
echo "$HOST_PID" > "$PID_FILE"

echo "   Host session PID: $HOST_PID"
echo "   Waiting for bootstrap output..."

# Wait for bootstrap JSON to appear (timeout after 15 seconds)
TIMEOUT=15
ELAPSED=0
while [[ $ELAPSED -lt $TIMEOUT ]]; do
  if [[ -f "$BOOTSTRAP_FILE" ]] && grep -q "session_id" "$BOOTSTRAP_FILE" 2>/dev/null; then
    break
  fi
  sleep 1
  ELAPSED=$((ELAPSED + 1))

  # Check if process is still running
  if ! kill -0 "$HOST_PID" 2>/dev/null; then
    echo "❌ Error: Host process died unexpectedly" >&2
    echo "   Check logs at: $LOG_DIR/session-output.log" >&2
    exit 1
  fi
done

if [[ $ELAPSED -ge $TIMEOUT ]]; then
  echo "❌ Error: Timeout waiting for bootstrap output" >&2
  echo "   Process is still running (PID: $HOST_PID)" >&2
  echo "   Check logs at: $LOG_DIR/session-output.log" >&2
  kill "$HOST_PID" 2>/dev/null || true
  exit 1
fi

# Parse session credentials
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if ! "$SCRIPT_DIR/parse-bootstrap.sh" "$BOOTSTRAP_FILE" > "$SESSION_CREDS_FILE"; then
  echo "❌ Error: Failed to parse bootstrap output" >&2
  kill "$HOST_PID" 2>/dev/null || true
  exit 1
fi

# Source the credentials and display them
source "$SESSION_CREDS_FILE"

echo "✅ Beach host session started successfully!"
echo ""
echo "   Session ID:  $SESSION_ID"
echo "   Passcode:    $PASSCODE"
echo "   Server:      $SESSION_SERVER"
echo "   PID:         $HOST_PID"
echo ""
echo "   Credentials saved to: $SESSION_CREDS_FILE"
echo ""
echo "🎮 Pong demo is running. Wait 2-3 seconds for initialization..."
sleep 3

echo "✅ Session ready for testing!"

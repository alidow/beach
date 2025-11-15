#!/usr/bin/env bash
set -euo pipefail

require() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "Missing dependency: $1" >&2
    exit 1
  }
}

require jq
require rg
require timeout

STACK_SERVICES=${STACK_SERVICES:-"beach-gate beach-road beach-manager"}
SESSION_SERVER=${SESSION_SERVER:-"http://localhost:4132/"}
LOG_DIR=${LOG_DIR:-"$HOME/beach-debug"}
SMOKE_TIMEOUT=${SMOKE_TIMEOUT:-180}
RUST_LOG=${RUST_LOG:-"info,beach_client_core::transport::webrtc=trace"}

mkdir -p "$LOG_DIR"
HOST_LOG="$LOG_DIR/fastpath-smoke-host.log"
BOOTSTRAP="$LOG_DIR/fastpath-smoke-bootstrap.json"
rm -f "$HOST_LOG" "$BOOTSTRAP"

cleanup() {
  if [[ -n "${HOST_PID:-}" ]]; then
    kill "$HOST_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT

echo "Ensuring stack services are running: $STACK_SERVICES"
docker compose up -d $STACK_SERVICES >/dev/null

echo "Launching throwaway host (logs: $HOST_LOG)"
(
  cd "$(dirname "$0")/.."
  RUST_LOG="$RUST_LOG" cargo run --bin beach -- \
    --log-level info \
    --log-file "$HOST_LOG" \
    --session-server "$SESSION_SERVER" \
    host \
    --bootstrap-output json \
    --wait \
    -- /bin/sh -c 'echo fastpath-smoke; sleep 120'
) | tee "$BOOTSTRAP" &
HOST_PID=$!

SESSION_ID=""
JOIN_CODE=""
BOOTSTRAP_JSON=""
for _ in $(seq 1 "$SMOKE_TIMEOUT"); do
  if [[ -s "$BOOTSTRAP" ]]; then
    BOOTSTRAP_JSON=$(head -n 1 "$BOOTSTRAP")
    if jq -e '.' >/dev/null 2>&1 <<<"$BOOTSTRAP_JSON"; then
      SESSION_ID=$(jq -r '.session_id' <<<"$BOOTSTRAP_JSON")
      JOIN_CODE=$(jq -r '.join_code' <<<"$BOOTSTRAP_JSON")
      break
    fi
  fi
  sleep 1
done

if [[ -z "$SESSION_ID" || "$SESSION_ID" == "null" ]]; then
  echo "Timed out waiting for bootstrap output ($BOOTSTRAP)" >&2
  exit 1
fi

echo "Session ID: $SESSION_ID"
echo "Join Code: $JOIN_CODE"
echo "→ Attach this session to your Private Beach (dashboard or API) within the next $SMOKE_TIMEOUT seconds."

if timeout "$SMOKE_TIMEOUT" bash -c '
  LOG_FILE="$1"
  SESSION="$2"
  while true; do
    if rg -q "fast path controller channel ready.*session_id=$SESSION" "$LOG_FILE"; then
      exit 0
    fi
    sleep 1
  done
' _ "$HOST_LOG" "$SESSION_ID"; then
  echo "✅ Fast-path controller channel established for $SESSION_ID"
else
  echo "❌ Fast-path channel never became ready. Check $HOST_LOG and docker logs beach-manager." >&2
  exit 1
fi

cleanup
trap - EXIT
exit 0

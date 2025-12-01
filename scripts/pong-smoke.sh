#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/pong-smoke.sh [--beach-id <id>] [--log-root <dir>] [--force-turn]

Runs the browserless Pong smoke: restarts the docker stack, launches pong-stack
with setup, then verifies ball/paddle motion and transport health from the
state traces under /tmp/pong-stack/<timestamp>.

Options:
  --beach-id <id>   Reuse a specific private beach id (default: random UUID).
  --log-root <dir>  Override the pong-stack log root (default: /tmp/pong-stack).
  --force-turn      Run with NEXT_PUBLIC_FORCE_TURN=1 for TURN-only validation.
  -h, --help        Show this help text.
USAGE
}

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LOG_ROOT="/tmp/pong-stack"
FORCE_TURN=0
BEACH_ID="${PONG_BEACH_ID:-}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --beach-id)
      BEACH_ID="$2"
      shift 2
      ;;
    --log-root)
      LOG_ROOT="$2"
      shift 2
      ;;
    --force-turn)
      FORCE_TURN=1
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage
      exit 1
      ;;
  esac
done

if [[ -z "$BEACH_ID" ]]; then
  if command -v uuidgen >/dev/null 2>&1; then
    BEACH_ID="$(uuidgen)"
  else
    BEACH_ID="$(date +%s)"
  fi
fi

echo "[pong-smoke] using beach id: $BEACH_ID"
export PONG_LOG_ROOT="$LOG_ROOT"
unset PONG_LOG_DIR
# Manager can take time to build on cold starts; poll longer to avoid early resets.
export PONG_STACK_MANAGER_HEALTH_ATTEMPTS="${PONG_STACK_MANAGER_HEALTH_ATTEMPTS:-240}"
export PONG_STACK_MANAGER_HEALTH_INTERVAL="${PONG_STACK_MANAGER_HEALTH_INTERVAL:-3}"

if [[ $FORCE_TURN -eq 1 ]]; then
  export NEXT_PUBLIC_FORCE_TURN=1
  echo "[pong-smoke] forcing TURN (NEXT_PUBLIC_FORCE_TURN=1)"
fi

echo "[pong-smoke] restarting docker compose stack via scripts/dockerup..."
direnv exec "$ROOT" "$ROOT/scripts/dockerup"

echo "[pong-smoke] launching pong stack..."
direnv exec "$ROOT" env RUST_LOG="${RUST_LOG:-info,webrtc=trace,webrtc::ice_transport=trace,webrtc::peer_connection=trace}" \
  "$ROOT/apps/private-beach/demo/pong/tools/pong-stack.sh" --create-beach --setup-beach start "$BEACH_ID"

echo "[pong-smoke] copying logs from beach-manager container..."
rm -rf "$LOG_ROOT"
docker compose cp beach-manager:"$LOG_ROOT" "$LOG_ROOT"

echo "[pong-smoke] verifying state traces..."
direnv exec "$ROOT" python3 "$ROOT/scripts/pong_smoke_verify.py" \
  --log-root "$LOG_ROOT" \
  --beach-id "$BEACH_ID"

echo "[pong-smoke] completed successfully."

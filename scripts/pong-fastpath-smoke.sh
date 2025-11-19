#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/pong-fastpath-smoke.sh --private-beach <uuid> [options]

The script launches the pong stack inside beach-manager, waits for fast-path
transport readiness, collects detailed traces (logs/frame dumps/ball traces),
and verifies that the automation agent observes real ball exchanges.

Options:
  --private-beach <uuid>   Required Private Beach identifier to target.
  --duration <seconds>     How long to let the sessions run before sampling
                           traces (default: 45).
  --no-setup               Skip the automatic layout seeding step.
  --artifacts <path>       Host directory to store copied logs/traces.
USAGE
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "Missing required command: $1" >&2
    exit 1
  }
}

# --------------------------------------------------------------------------- CLI
PRIVATE_BEACH_ID=""
RUN_DURATION=45
DO_SETUP=1
HOST_ARTIFACTS=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --private-beach)
      PRIVATE_BEACH_ID=${2:-}
      shift 2
      ;;
    --duration)
      RUN_DURATION=${2:-}
      shift 2
      ;;
    --no-setup)
      DO_SETUP=0
      shift
      ;;
    --artifacts)
      HOST_ARTIFACTS=${2:-}
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 1
      ;;
  esac
done

if [[ -z "$PRIVATE_BEACH_ID" ]]; then
  echo "--private-beach is required." >&2
  usage
  exit 1
fi

require_cmd direnv
require_cmd docker
require_cmd jq
require_cmd rg
require_cmd timeout

REPO_ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$REPO_ROOT"

RUN_ID=$(date +"%Y%m%d-%H%M%S")
CONTAINER_LOG_DIR="/tmp/pong-fastpath-smoke-$RUN_ID"
HOST_ARTIFACT_DIR=${HOST_ARTIFACTS:-"$REPO_ROOT/logs/pong-fastpath-smoke/$RUN_ID"}
mkdir -p "$HOST_ARTIFACT_DIR"

PONG_STACK="$REPO_ROOT/apps/private-beach/demo/pong/tools/pong-stack.sh"

# Ensure we always shut down pong players/agent when done.
cleanup() {
  direnv exec . "$PONG_STACK" stop >/dev/null 2>&1 || true
}
trap cleanup EXIT

# ---------------------------------------------------------------- utilities -----
docker_exec() {
  direnv exec . docker compose "$@"
}

wait_for_log() {
  local pattern="$1"
  local label="$2"
  local timeout_sec="$3"
  local start_ts
  start_ts=$(date +%s)
  while true; do
    if docker_exec exec -T beach-manager rg -q -- "$pattern" "$CONTAINER_LOG_DIR/agent.log" >/dev/null 2>&1; then
      echo "✔ $label"
      return 0
    fi
    local now
    now=$(date +%s)
    if (( now - start_ts > timeout_sec )); then
      echo "✖ Timed out waiting for '$label' in agent log." >&2
      return 1
    fi
    sleep 2
  done
}

copy_artifacts() {
  local dest="$1"
  mkdir -p "$dest"
  docker_exec cp "beach-manager:$CONTAINER_LOG_DIR/." "$dest/"
}

verify_agent_log() {
  local agent_log="$1"
  local failures=0

  if ! rg -q "fast-path ready for lhs" "$agent_log"; then
    echo "✖ fast-path readiness for lhs missing in $agent_log" >&2
    failures=$((failures + 1))
  fi
  if ! rg -q "fast-path ready for rhs" "$agent_log"; then
    echo "✖ fast-path readiness for rhs missing in $agent_log" >&2
    failures=$((failures + 1))
  fi
  if ! rg -q "players ready; controller commands running" "$agent_log"; then
    echo "✖ Agent never reported players ready" >&2
    failures=$((failures + 1))
  fi
  if ! rg -q "score update -" "$agent_log"; then
    echo "✖ No score updates observed (ball may not have crossed between terminals)" >&2
    failures=$((failures + 1))
  fi

  local watchdog_count
  watchdog_count=$(rg -c "ball watchdog forced serve" "$agent_log" || true)
  local allowed=${PONG_SMOKE_WATCHDOG_MAX:-2}
  if [[ "$watchdog_count" -gt "$allowed" ]]; then
    echo "✖ Detected $watchdog_count watchdog serves (allowed $allowed); fast-path likely not delivering terminal frames." >&2
    failures=$((failures + 1))
  fi

  return "$failures"
}

verify_ball_traces() {
  local trace_dir="$1"
  local failures=0
  for side in lhs rhs; do
    local trace_file="$trace_dir/ball-trace-$side.jsonl"
    if [[ ! -s "$trace_file" ]]; then
      echo "✖ Missing ball trace for $side ($trace_file)" >&2
      failures=$((failures + 1))
      continue
    fi
    local count
    count=$(wc -l <"$trace_file")
    if (( count < 5 )); then
      echo "✖ Too few samples in $trace_file ($count lines)" >&2
      failures=$((failures + 1))
    fi
  done
  return "$failures"
}

# ------------------------------------------------------------------- smoke run --
echo "Preparing pong fast-path smoke (run $RUN_ID)"
echo "Container log dir: $CONTAINER_LOG_DIR"
echo "Artifacts will be copied to: $HOST_ARTIFACT_DIR"

export PONG_LOG_DIR="$CONTAINER_LOG_DIR"
export PONG_FRAME_DUMP_DIR="$CONTAINER_LOG_DIR/frame-dumps"
export PONG_FRAME_DUMP_INTERVAL="${PONG_FRAME_DUMP_INTERVAL:-0.5}"
export PONG_BALL_TRACE_DIR="$CONTAINER_LOG_DIR/ball-traces"
export PONG_COMMAND_TRACE_DIR="$CONTAINER_LOG_DIR/commands"
export PONG_LOG_LEVEL="${PONG_LOG_LEVEL:-debug}"
export PONG_AGENT_LOG_LEVEL="${PONG_AGENT_LOG_LEVEL:-debug}"
export PONG_CODES_WAIT=6

echo "Stopping any existing pong demo processes..."
direnv exec . "$PONG_STACK" stop >/dev/null 2>&1 || true

stack_args=()
if [[ "$DO_SETUP" -eq 1 ]]; then
  stack_args+=(--setup-beach)
fi
stack_args+=(start "$PRIVATE_BEACH_ID")

echo "Launching pong stack..."
if ! direnv exec . env PONG_LOG_DIR="$PONG_LOG_DIR" \
  PONG_FRAME_DUMP_DIR="$PONG_FRAME_DUMP_DIR" \
  PONG_FRAME_DUMP_INTERVAL="$PONG_FRAME_DUMP_INTERVAL" \
  PONG_BALL_TRACE_DIR="$PONG_BALL_TRACE_DIR" \
  PONG_COMMAND_TRACE_DIR="$PONG_COMMAND_TRACE_DIR" \
  PONG_LOG_LEVEL="$PONG_LOG_LEVEL" \
  PONG_AGENT_LOG_LEVEL="$PONG_AGENT_LOG_LEVEL" \
  "$PONG_STACK" "${stack_args[@]}"; then
  echo "Failed to start pong stack." >&2
  exit 1
fi

echo "Waiting for fast-path readiness markers..."
wait_for_log "fast-path ready for lhs" "lhs fast-path ready" 90
wait_for_log "fast-path ready for rhs" "rhs fast-path ready" 90
wait_for_log "players ready; controller commands running" "agent commands active" 90

echo "Letting the demo run for $RUN_DURATION seconds..."
sleep "$RUN_DURATION"

echo "Collecting logs from container..."
copy_artifacts "$HOST_ARTIFACT_DIR/container"

AGENT_LOG="$HOST_ARTIFACT_DIR/container/agent.log"
BALL_TRACE_DIR="$HOST_ARTIFACT_DIR/container/ball-traces"

failures=0
if ! verify_agent_log "$AGENT_LOG"; then
  failures=$((failures + 1))
fi
if ! verify_ball_traces "$BALL_TRACE_DIR"; then
  failures=$((failures + 1))
fi

echo "Stopping pong stack..."
direnv exec . "$PONG_STACK" stop >/dev/null 2>&1 || true

if [[ "$failures" -gt 0 ]]; then
  echo ""
  echo "❌ Pong fast-path smoke FAILED. Inspect artifacts in $HOST_ARTIFACT_DIR" >&2
  exit 1
fi

echo ""
echo "✅ Pong fast-path smoke PASSED."
echo "Artifacts available at: $HOST_ARTIFACT_DIR"

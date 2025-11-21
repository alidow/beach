#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/pong-fastpath-smoke.sh [options]

The script launches the pong stack inside beach-manager, waits for fast-path
transport readiness, collects detailed traces (logs/frame dumps/ball traces),
and verifies that the automation agent observes real ball exchanges.

Options:
  --private-beach <uuid>   Use an existing Private Beach instead of creating one.
  --profile <name>         Beach CLI profile to use for manager auth (default: local).
  --manager-url <url>      Override the manager base URL (default: http://localhost:8080).
  --duration <seconds>     How long to let the sessions run before sampling
                           traces (default: 45).
  --no-setup               Skip the automatic layout seeding step.
  --keep-beach             Do not delete the auto-created private beach.
  --artifacts <path>       Host directory to store copied logs/traces
                           (default: temp/pong-fastpath-smoke/<timestamp>).
  --skip-stack             Assume docker compose services are already up (no rebuild/down/up).
USAGE
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "Missing required command: $1" >&2
    exit 1
  }
}

resolve_cli_token() {
  local profile="$1"
  python3 - "$profile" <<'PY'
import os, sys, json
try:
    import tomllib as toml
except ModuleNotFoundError:
    import tomli as toml  # type: ignore

profile = sys.argv[1]
path = os.path.expanduser("~/.beach/credentials")
try:
    with open(path, "rb") as fh:
        data = toml.load(fh)
    token = (
        data.get("profiles", {})
        .get(profile, {})
        .get("access_token", {})
        .get("token")
    )
    if token:
        sys.stdout.write(token)
        sys.exit(0)
except FileNotFoundError:
    pass
sys.exit(1)
PY
}

ensure_cli_login() {
  local profile="$1"
  echo "[pong-smoke] launching beach login for profile '$profile'..." >&2
  (cd "$REPO_ROOT" && cargo run --bin beach -- login --name "$profile" --force) >&2
}

obtain_manager_token() {
  local profile="$1"
  if token=$(resolve_cli_token "$profile" 2>/dev/null); then
    printf '%s\n' "$token"
    return 0
  fi
  ensure_cli_login "$profile" || return 1
  resolve_cli_token "$profile"
}

rand_suffix() {
  python3 - <<'PY'
import uuid
print(uuid.uuid4().hex[:10])
PY
}

create_private_beach() {
  local token="$1"
  local name="Pong Smoke $(date +%H:%M:%S)"
  local slug="pong-smoke-$(rand_suffix)"
  local payload
  payload=$(jq -n --arg name "$name" --arg slug "$slug" '{name:$name, slug:$slug}')
  local resp
  if ! resp=$(curl -sS --fail --max-time 15 \
      -H "Authorization: Bearer $token" \
      -H "Content-Type: application/json" \
      -X POST \
      "$MANAGER_URL/private-beaches" \
      --data "$payload" \
      2>/tmp/pong-smoke-create.err); then
    echo "[pong-smoke] failed to create private beach via manager API (timeout or error)" >&2
    cat /tmp/pong-smoke-create.err >&2 || true
    rm -f /tmp/pong-smoke-create.err
    return 1
  fi
  rm -f /tmp/pong-smoke-create.err
  local id
  id=$(jq -r '.id // empty' <<<"$resp")
  if [[ -z "$id" ]]; then
    echo "[pong-smoke] manager response missing id: $resp" >&2
    return 1
  fi
  echo "$id"
}

delete_private_beach() {
  local beach_id="$1"
  [[ -z "$beach_id" ]] && return 0
  curl -sS -X DELETE \
    -H "Authorization: Bearer $MANAGER_TOKEN" \
    "$MANAGER_URL/private-beaches/$beach_id" >/dev/null 2>&1 || true
}

# --------------------------------------------------------------------------- CLI
PRIVATE_BEACH_ID=""
RUN_DURATION=45
DO_SETUP=1
HOST_ARTIFACTS=""
CLI_PROFILE="local"
KEEP_BEACH=0
CUSTOM_MANAGER_URL=""
SKIP_STACK=0

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
    --profile)
      CLI_PROFILE=${2:-}
      shift 2
      ;;
    --keep-beach)
      KEEP_BEACH=1
      shift
      ;;
    --skip-stack)
      SKIP_STACK=1
      shift
      ;;
    --manager-url)
      CUSTOM_MANAGER_URL=${2:-}
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

require_cmd direnv
require_cmd docker
require_cmd jq
require_cmd rg
require_cmd timeout
require_cmd curl

REPO_ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$REPO_ROOT"

if [[ -z "${BEACH_ICE_PUBLIC_IP:-}" || -z "${BEACH_ICE_PUBLIC_HOST:-}" ]]; then
  eval "$(direnv export bash)" >/dev/null
fi

RUN_ID=$(date +"%Y%m%d-%H%M%S")
CONTAINER_LOG_DIR="/tmp/pong-fastpath-smoke-$RUN_ID"
HOST_ARTIFACT_DIR=${HOST_ARTIFACTS:-"$REPO_ROOT/temp/pong-fastpath-smoke/$RUN_ID"}
mkdir -p "$HOST_ARTIFACT_DIR"
MANAGER_HEALTH_TIMEOUT=${PONG_SMOKE_MANAGER_HEALTH_SECS:-120}
BOOTSTRAP_TIMEOUT=${PONG_SMOKE_BOOTSTRAP_TIMEOUT:-240}
READY_TIMEOUT=${PONG_SMOKE_READY_TIMEOUT:-15}
TAIL_TIMEOUT=${PONG_SMOKE_TAIL_TIMEOUT:-$READY_TIMEOUT}
STACK_START_TIMEOUT=${PONG_SMOKE_STACK_TIMEOUT:-150}
RUN_DURATION=${PONG_SMOKE_RUN_DURATION:-$RUN_DURATION}

MANAGER_URL=${CUSTOM_MANAGER_URL:-${PRIVATE_BEACH_MANAGER_URL:-http://localhost:8080}}
STACK_ENV_SESSION_SERVER=${STACK_ENV_SESSION_SERVER:-http://beach-road:4132}
STACK_ENV_WATCHDOG=${STACK_ENV_WATCHDOG:-10.0}
STACK_ENV_STDOUT_LOG=${STACK_ENV_STDOUT_LOG:-trace}
STACK_ENV_FILE_LOG=${STACK_ENV_FILE_LOG:-trace}
STACK_ENV_TRACE_DEPS=${STACK_ENV_TRACE_DEPS:-1}
STACK_ENV_ICE_IP=${STACK_ENV_ICE_IP:-${BEACH_HOST_LAN_IP:-${BEACH_ICE_PUBLIC_IP:-}}}
STACK_ENV_ICE_HOST=${STACK_ENV_ICE_HOST:-${BEACH_ICE_PUBLIC_HOST:-host.docker.internal}}
if [[ -z "$STACK_ENV_ICE_IP" ]]; then
  echo "[pong-smoke] could not determine BEACH_ICE_PUBLIC_IP; run direnv allow or pass STACK_ENV_ICE_IP." >&2
  exit 1
fi
echo "[pong-smoke] using ICE hints ip=$STACK_ENV_ICE_IP host=$STACK_ENV_ICE_HOST"

PONG_STACK="$REPO_ROOT/apps/private-beach/demo/pong/tools/pong-stack.sh"
CREATED_BEACH_ID=""
MANAGER_TOKEN=""

# Ensure we always shut down pong players/agent when done.
cleanup() {
  stop_agent_tail || true
  direnv exec . "$PONG_STACK" stop >/dev/null 2>&1 || true
  if [[ -n "$CREATED_BEACH_ID" && "$KEEP_BEACH" -eq 0 ]]; then
    delete_private_beach "$CREATED_BEACH_ID" || true
  fi
}
trap cleanup EXIT

# ---------------------------------------------------------------- utilities -----
docker_exec() {
  direnv exec . docker compose "$@"
}

ensure_manager_container_running() {
  if ! direnv exec . docker ps --format '{{.Names}}' | rg -q '^beach-manager(-[0-9]+)?$' >/dev/null 2>&1; then
    echo "[pong-smoke] beach-manager container is not running; start docker compose or rerun without --skip-stack." >&2
    exit 1
  fi
}

bootstrap_ready() {
  local side="$1"
  docker_exec exec -T beach-manager python3 - "$CONTAINER_LOG_DIR" "$side" <<'PY'
import json
import os
import sys

log_dir = sys.argv[1]
role = sys.argv[2]
path = os.path.join(log_dir, f"bootstrap-{role}.json")
try:
    with open(path, "r", encoding="utf-8") as fh:
        for raw in fh:
            raw = raw.strip()
            if not raw or raw[0] not in "{[":
                continue
            try:
                payload = json.loads(raw)
            except json.JSONDecodeError:
                continue
            session = (
                payload.get("session_id")
                or payload.get("sessionId")
                or payload.get("session")
            )
            if isinstance(session, dict):
                session = session.get("sessionId") or session.get("id")
            if session:
                sys.exit(0)
except FileNotFoundError:
    pass
sys.exit(1)
PY
}

wait_for_bootstrap_data() {
  local timeout="${1:-$BOOTSTRAP_TIMEOUT}"
  local start_ts
  start_ts=$(date +%s)
  local sides=("lhs" "rhs" "agent")
  while true; do
    local pending=()
    for side in "${sides[@]}"; do
      if ! bootstrap_ready "$side" >/dev/null 2>&1; then
        pending+=("$side")
      fi
    done
    if (( ${#pending[@]} == 0 )); then
      echo "[pong-smoke] bootstrap metadata ready for lhs, rhs, and agent."
      return 0
    fi
    local now
    now=$(date +%s)
    if (( now - start_ts >= timeout )); then
      echo "✖ Timed out waiting for bootstrap data (${pending[*]})" >&2
      return 1
    fi
    sleep 2
  done
}

wait_for_manager_health() {
  local attempt=0
  local interval=2
  local max_attempts=$((MANAGER_HEALTH_TIMEOUT / interval))
  if (( max_attempts < 1 )); then
    max_attempts=1
  fi
  while (( attempt < max_attempts )); do
    if curl -fsS "$MANAGER_URL/healthz" >/dev/null 2>&1; then
      echo "[pong-smoke] manager healthy at $MANAGER_URL"
      return 0
    fi
    attempt=$((attempt + 1))
    sleep "$interval"
  done
  local waited=$((attempt * interval))
  echo "[pong-smoke] manager did not become healthy after ${waited}s (timeout configured via PONG_SMOKE_MANAGER_HEALTH_SECS=$MANAGER_HEALTH_TIMEOUT)" >&2
  return 1
}

restart_stack() {
  local dockerdown="$REPO_ROOT/scripts/dockerdown"
  if [[ ! -x "$dockerdown" ]]; then
    echo "[pong-smoke] missing scripts/dockerdown helper" >&2
    return 1
  fi
  echo "[pong-smoke] resetting docker compose stack (postgres + services)..."
  direnv exec . "$dockerdown" --postgres-only
  direnv exec . docker compose down
  direnv exec . env BEACH_SESSION_SERVER="$STACK_ENV_SESSION_SERVER" \
    PONG_WATCHDOG_INTERVAL="$STACK_ENV_WATCHDOG" \
    BEACH_ICE_PUBLIC_IP="$STACK_ENV_ICE_IP" \
    BEACH_ICE_PUBLIC_HOST="$STACK_ENV_ICE_HOST" \
    docker compose build beach-manager
  direnv exec . sh -c "BEACH_SESSION_SERVER='$STACK_ENV_SESSION_SERVER' \
    PONG_WATCHDOG_INTERVAL='$STACK_ENV_WATCHDOG' \
    BEACH_MANAGER_STDOUT_LOG='$STACK_ENV_STDOUT_LOG' \
    BEACH_MANAGER_FILE_LOG='$STACK_ENV_FILE_LOG' \
    BEACH_MANAGER_TRACE_DEPS='$STACK_ENV_TRACE_DEPS' \
    BEACH_ICE_PUBLIC_IP='$STACK_ENV_ICE_IP' \
    BEACH_ICE_PUBLIC_HOST='$STACK_ENV_ICE_HOST' \
    docker compose up -d"
  wait_for_manager_health
}

wait_for_log() {
  local pattern="$1"
  local label="$2"
  local timeout_sec="$3"
  local start_ts
  start_ts=$(date +%s)
  while true; do
    # Use grep -aF to tolerate ANSI/TTY bytes in the agent log.
    if docker_exec exec -T beach-manager sh -c "grep -aF -- \"$pattern\" \"$CONTAINER_LOG_DIR/agent.log\" >/dev/null 2>&1"; then
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

wait_for_readiness_markers() {
  local patterns=(
    "fast-path ready for lhs"
    "fast-path ready for rhs"
    "players ready; controller commands"
  )
  local labels=(
    "lhs fast-path ready"
    "rhs fast-path ready"
    "agent commands active"
  )
  local -a pids=()
  local count=${#patterns[@]}
  local i
  for ((i = 0; i < count; i++)); do
    wait_for_log "${patterns[$i]}" "${labels[$i]}" "$READY_TIMEOUT" &
    pids[$i]=$!
  done

  local failure=0
  for ((i = 0; i < count; i++)); do
    if ! wait "${pids[$i]}" >/dev/null 2>&1; then
      failure=1
      break
    fi
  done

  if (( failure )); then
    for pid in "${pids[@]}"; do
      if [[ -n "$pid" ]]; then
        kill "$pid" >/dev/null 2>&1 || true
        wait "$pid" >/dev/null 2>&1 || true
      fi
    done
    return 1
  fi

  return 0
}

copy_artifacts() {
  local dest="$1"
  mkdir -p "$dest"
  docker_exec cp "beach-manager:$CONTAINER_LOG_DIR/." "$dest/"

  find "$dest" -maxdepth 1 -type f -name "*.log" -print0 | while IFS= read -r -d '' logfile; do
    local base
    base=$(basename "$logfile" .log)
    mv "$logfile" "$dest/${base}-${RUN_ID}.log"
  done
}

start_agent_tail() {
  local output="$1"
  mkdir -p "$(dirname "$output")"
  (
    set +e
    timeout --foreground "$TAIL_TIMEOUT" \
      direnv exec . docker compose exec -T beach-manager env SMOKE_AGENT_LOG="$CONTAINER_LOG_DIR/agent.log" bash -lc '
        set -euo pipefail
        logfile="${SMOKE_AGENT_LOG:?missing agent log path}"
        mkdir -p "$(dirname "$logfile")"
        touch "$logfile"
        tail -n0 -F "$logfile"
      '
  ) >"$output" 2>&1 &
  AGENT_TAIL_PID=$!
}

stop_agent_tail() {
  if [[ -n "${AGENT_TAIL_PID:-}" ]]; then
    kill "$AGENT_TAIL_PID" >/dev/null 2>&1 || true
    wait "$AGENT_TAIL_PID" >/dev/null 2>&1 || true
    unset AGENT_TAIL_PID
  fi
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
  if ! rg -q "players ready; controller commands" "$agent_log"; then
    echo "✖ Agent never reported players ready" >&2
    failures=$((failures + 1))
  fi
  if ! rg -q "score update -" "$agent_log"; then
    echo "✖ No score updates observed (ball may not have crossed between terminals)" >&2
    failures=$((failures + 1))
  fi

  local watchdog_count
  watchdog_count=$(rg -c "ball watchdog forced serve" "$agent_log" || true)
  local allowed=${PONG_SMOKE_WATCHDOG_MAX:-6}
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

# Acquire auth token and prepare stack before running smoke
MANAGER_TOKEN=$(obtain_manager_token "$CLI_PROFILE" 2>/dev/null || true)
if [[ -z "$MANAGER_TOKEN" ]]; then
  echo "Unable to resolve manager token for profile '$CLI_PROFILE'." >&2
  exit 1
fi
# Keep players as public host sessions; agent runs against the private beach.
export PONG_DISABLE_HOST_TOKEN=1

if [[ "$SKIP_STACK" -eq 0 ]]; then
  restart_stack
else
  echo "[pong-smoke] skipping docker compose restart (per --skip-stack)."
  ensure_manager_container_running
  wait_for_manager_health
fi

if [[ -z "$PRIVATE_BEACH_ID" ]]; then
  echo "Creating temporary private beach via $MANAGER_URL..."
  if ! PRIVATE_BEACH_ID=$(create_private_beach "$MANAGER_TOKEN"); then
    echo "Failed to create private beach; aborting." >&2
    exit 1
  fi
  CREATED_BEACH_ID="$PRIVATE_BEACH_ID"
  echo "Created private beach: $CREATED_BEACH_ID"
  if [[ "$KEEP_BEACH" -eq 1 ]]; then
    echo "Will retain temporary private beach after the test completes."
  else
    echo "Temporary private beach will be deleted after the test completes."
  fi
else
  echo "Using provided private beach: $PRIVATE_BEACH_ID"
fi

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
export PONG_CODES_WAIT="${PONG_CODES_WAIT:-6}"
export RUST_LOG="${PONG_SMOKE_RUST_LOG:-info,controller=debug,webrtc=trace,webrtc::ice_transport=trace,webrtc::peer_connection=trace}"

echo "Stopping any existing pong demo processes..."
direnv exec . "$PONG_STACK" stop >/dev/null 2>&1 || true

stack_args=()
if [[ "$DO_SETUP" -eq 1 ]]; then
  stack_args+=(--setup-beach)
fi
stack_args+=(start "$PRIVATE_BEACH_ID")

echo "Launching pong stack..."
stack_status=0
if ! timeout --foreground "$STACK_START_TIMEOUT" direnv exec . env PONG_LOG_DIR="$PONG_LOG_DIR" \
  PONG_FRAME_DUMP_DIR="$PONG_FRAME_DUMP_DIR" \
  PONG_FRAME_DUMP_INTERVAL="$PONG_FRAME_DUMP_INTERVAL" \
  PONG_BALL_TRACE_DIR="$PONG_BALL_TRACE_DIR" \
  PONG_COMMAND_TRACE_DIR="$PONG_COMMAND_TRACE_DIR" \
  PONG_LOG_LEVEL="$PONG_LOG_LEVEL" \
  PONG_AGENT_LOG_LEVEL="$PONG_AGENT_LOG_LEVEL" \
  "$PONG_STACK" "${stack_args[@]}"; then
  stack_status=$?
fi
if (( stack_status != 0 )); then
  if [[ $stack_status -eq 124 ]]; then
    echo "Failed to start pong stack within ${STACK_START_TIMEOUT}s (timeout)." >&2
  else
    echo "[pong-smoke] pong-stack start failed with exit code $stack_status; inspect logs in $CONTAINER_LOG_DIR." >&2
  fi
  copy_artifacts "$HOST_ARTIFACT_DIR/container" || true
  exit 1
fi

AGENT_TAIL_LOG="$HOST_ARTIFACT_DIR/agent-tail-${RUN_ID}.log"
echo "[pong-smoke] tailing agent log for ${TAIL_TIMEOUT}s -> $AGENT_TAIL_LOG"
start_agent_tail "$AGENT_TAIL_LOG"

echo "Waiting for session bootstrap data..."
if ! wait_for_bootstrap_data "$BOOTSTRAP_TIMEOUT"; then
  exit 1
fi

echo "Waiting for fast-path readiness markers..."
readiness_failed=0
if ! wait_for_readiness_markers; then
  readiness_failed=1
fi
stop_agent_tail

if (( readiness_failed )); then
  echo "[pong-smoke] readiness markers not observed; skipping extended runtime window." >&2
else
  echo "Letting the demo run for $RUN_DURATION seconds..."
  sleep "$RUN_DURATION"
fi

echo "Collecting logs from container..."
copy_artifacts "$HOST_ARTIFACT_DIR/container"
echo "[pong-smoke] dumping beach-manager logs to $HOST_ARTIFACT_DIR/beach-manager.log"
direnv exec . sh -c "docker logs beach-manager > \"$HOST_ARTIFACT_DIR/beach-manager.log\" 2>&1" || true

# Also grab the manager runtime log files from inside the container for forwarder traces.
direnv exec . sh -c "docker exec beach-manager sh -c 'tar -C /app/temp -cf - bm-run.log bm-run-*.log manager-latest.log manager-tail.log 2>/dev/null || true' | tar -xvf - -C \"$HOST_ARTIFACT_DIR\" >/dev/null 2>&1" || true
if [[ -f "$REPO_ROOT/logs/beach-manager/beach-manager.log" ]]; then
  cp "$REPO_ROOT/logs/beach-manager/beach-manager.log" "$HOST_ARTIFACT_DIR/beach-manager-file.log" || true
fi

AGENT_LOG="$HOST_ARTIFACT_DIR/container/agent-${RUN_ID}.log"
BALL_TRACE_DIR="$HOST_ARTIFACT_DIR/container/ball-traces"

failures=0
if (( readiness_failed )); then
  failures=$((failures + 1))
fi
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
if [[ -n "$CREATED_BEACH_ID" && "$KEEP_BEACH" -eq 1 ]]; then
  echo "Temporary private beach retained for inspection: $CREATED_BEACH_ID"
fi
if [[ "$SKIP_STACK" -eq 0 ]]; then
  echo "Docker compose services were restarted as part of this run."
fi

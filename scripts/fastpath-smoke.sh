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
FASTPATH_SMOKE_MODES=${FASTPATH_SMOKE_MODES:-"public,managed"}
FASTPATH_SMOKE_LOCAL_ICE=${FASTPATH_SMOKE_LOCAL_ICE:-'[{"urls":["stun:host.docker.internal:3478","stun:127.0.0.1:3478"]}]'}
FASTPATH_SMOKE_GATE_PROFILE=${FASTPATH_SMOKE_GATE_PROFILE:-default}
FASTPATH_SMOKE_GATE_SKIP_LOGIN=${FASTPATH_SMOKE_GATE_SKIP_LOGIN:-0}
FASTPATH_SMOKE_PAYLOAD_LINES=${FASTPATH_SMOKE_PAYLOAD_LINES:-512}
FASTPATH_SMOKE_PAYLOAD_WIDTH=${FASTPATH_SMOKE_PAYLOAD_WIDTH:-128}

mkdir -p "$LOG_DIR"

cleanup() {
  if [[ -n "${HOST_PID:-}" ]]; then
    kill "$HOST_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT

echo "Ensuring stack services are running: $STACK_SERVICES"
docker compose up -d $STACK_SERVICES >/dev/null

wait_for_fastpath() {
  local log_file="$1"
  local session_id="$2"
  timeout "$SMOKE_TIMEOUT" bash -c '
    LOG_FILE="$1"
    SESSION="$2"
    while true; do
      if rg -q "fast path controller channel ready.*session_id=$SESSION" "$LOG_FILE"; then
        exit 0
      fi
      sleep 1
    done
  ' _ "$log_file" "$session_id"
}

ensure_gate_login() {
  if [[ "$FASTPATH_SMOKE_GATE_SKIP_LOGIN" == "1" ]]; then
    return
  fi

  local profile="$FASTPATH_SMOKE_GATE_PROFILE"
  echo "Checking Beach Auth profile '$profile' for gate-mode smoke..."
  local status_output=""
  if ! status_output=$(cargo run --bin beach -- auth status --profile "$profile" 2>&1); then
    status_output=""
  fi

  if grep -q "Profile '$profile' is not stored" <<<"$status_output" || grep -q "No Beach Auth profiles" <<<"$status_output"; then
    echo "No Beach Auth profile '$profile' found. Launching device login..."
    echo "Follow the prompts to complete login; the smoke test will resume afterwards."
    cargo run --bin beach -- auth login --name "$profile" --set-current --force
  else
    echo "Using existing Beach Auth profile '$profile'."
  fi
}

run_mode() {
  local mode="$1"
  local normalized="$mode"
  if [[ "$normalized" == "local" ]]; then
    normalized="public"
  fi
  local host_log="$LOG_DIR/fastpath-smoke-${mode}.log"
  local bootstrap="$LOG_DIR/fastpath-smoke-${mode}-bootstrap.json"
  rm -f "$host_log" "$bootstrap"

  echo ""
  echo "=== Running fast-path smoke test ($mode mode) ==="

  local env_prefix=()
  case "$normalized" in
    public)
      env_prefix+=("BEACH_PUBLIC_MODE=1")
      env_prefix+=("BEACH_ICE_SERVERS=$FASTPATH_SMOKE_LOCAL_ICE")
      ;;
    managed)
      env_prefix+=("BEACH_PUBLIC_MODE=0")
      env_prefix+=("BEACH_PROFILE=$FASTPATH_SMOKE_GATE_PROFILE")
      ;;
    *)
      echo "Unsupported mode '$mode'." >&2
      exit 1
      ;;
  esac

  echo "Launching throwaway host (logs: $host_log)"
  (
    cd "$(dirname "$0")/.."
    env "${env_prefix[@]+"${env_prefix[@]}"}" RUST_LOG="$RUST_LOG" cargo run --bin beach -- \
      --log-level info \
      --log-file "$host_log" \
      --session-server "$SESSION_SERVER" \
      host \
      --bootstrap-output json \
      --wait \
      -- /bin/sh -c "python3 -c \"import sys; line='fastpath-smoke-' * ${FASTPATH_SMOKE_PAYLOAD_WIDTH}; print('\\n'.join(line for _ in range(${FASTPATH_SMOKE_PAYLOAD_LINES})))\"; sleep 120"
  ) | tee "$bootstrap" &
  HOST_PID=$!

  local session_id=""
  local join_code=""
  local bootstrap_json=""
  for _ in $(seq 1 "$SMOKE_TIMEOUT"); do
    if [[ -s "$bootstrap" ]]; then
      bootstrap_json=$(head -n 1 "$bootstrap")
      if jq -e '.' >/dev/null <<<"$bootstrap_json"; then
        session_id=$(jq -r '.session_id' <<<"$bootstrap_json")
        join_code=$(jq -r '.join_code' <<<"$bootstrap_json")
        break
      fi
    fi
    sleep 1
  done

  if [[ -z "$session_id" || "$session_id" == "null" ]]; then
    echo "Timed out waiting for bootstrap output ($bootstrap)" >&2
    exit 1
  fi

  echo "Session ID: $session_id"
  echo "Join Code: $join_code"
  echo "→ Attach this session to your Private Beach (dashboard or API) within the next $SMOKE_TIMEOUT seconds."

  if wait_for_fastpath "$host_log" "$session_id"; then
    echo "✅ Fast-path controller channel established for $session_id"
  else
    echo "❌ Fast-path channel never became ready. Check $host_log and docker logs beach-manager." >&2
    exit 1
  fi

  case "$normalized" in
    public)
      if ! rg -q "using ICE servers from BEACH_ICE_SERVERS" "$host_log"; then
        echo "❌ Expected BEACH_ICE_SERVERS override to be used; not found in $host_log." >&2
        exit 1
      fi
      if rg -q "using TURN credentials from Beach Gate" "$host_log"; then
        echo "❌ Public mode must not request TURN credentials from Beach Gate." >&2
        exit 1
      fi
      ;;
    managed)
      if ! rg -q "using TURN credentials from Beach Gate" "$host_log"; then
        echo "❌ Expected Beach Gate TURN credentials in $host_log but log entry was missing." >&2
        exit 1
      fi
      ;;
  esac

  if ! rg -q 'payload_type="chunk"' "$host_log"; then
    echo "❌ Host never emitted chunked fast-path frames; inspect $host_log for controller/state publishing failures." >&2
    exit 1
  fi

  cleanup
  HOST_PID=""
}

IFS=',' read -ra MODE_LIST <<<"$FASTPATH_SMOKE_MODES"
if [[ "${#MODE_LIST[@]}" -eq 0 ]]; then
  echo "FASTPATH_SMOKE_MODES is empty; nothing to do." >&2
  exit 1
fi

for raw_mode in "${MODE_LIST[@]}"; do
  mode=$(echo "$raw_mode" | tr '[:upper:]' '[:lower:]' | xargs)
  if [[ "$mode" == "gate" ]]; then
    mode="managed"
  fi
  case "$mode" in
    public|local)
      run_mode "$mode"
      ;;
    managed)
      ensure_gate_login
      run_mode "$mode"
      ;;
    *)
      echo "Unknown mode '$raw_mode' in FASTPATH_SMOKE_MODES. Supported values: public, managed (and legacy aliases local, gate)." >&2
      exit 1
      ;;
  esac
done

trap - EXIT
exit 0

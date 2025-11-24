#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  pong-stack.sh [--setup-beach] [--create-beach] start [private-beach-id]
                                                        Start LHS, RHS, agent, and optionally auto-create/configure the beach layout.
  pong-stack.sh start lhs                                   Start only the LHS player.
  pong-stack.sh start rhs                                   Start only the RHS player.
  pong-stack.sh start agent <pb-id>                         Start only the mock agent.
  pong-stack.sh stop                                        Stop all pong demo processes running in the service.
  pong-stack.sh codes                                       Print current session IDs and passcodes from logs.

Options:
  --setup-beach         After launching lhs/rhs/agent, configure the private beach layout
                        via the manager session-graph API and verify that the attachments
                        and controller pairings succeeded.
  --create-beach        When starting the full stack, create a new private beach via manager
                        if no beach id is provided. Requires manager token scopes.

Environment variables:
  PONG_DOCKER_SERVICE   Docker compose service name (default: beach-manager)
  PRIVATE_BEACH_MANAGER_URL  Manager URL inside the container (default: http://localhost:8080)
  PONG_SESSION_SERVER   Beach session server URL (default: http://localhost:4132/)
  PONG_LOG_DIR          Log directory inside the container (default: /tmp/pong-stack)
  PONG_CODES_WAIT       Seconds to wait before printing session codes (default: 8)
  PONG_STACK_CONTAINER_ICE_IP    Force BEACH_ICE_PUBLIC_IP inside the container (default: unset)
  PONG_STACK_CONTAINER_ICE_HOST  Force BEACH_ICE_PUBLIC_HOST inside the container (default: unset)
  PONG_STACK_MANAGER_HEALTH_ATTEMPTS  Attempts to poll manager /healthz before start (default: 30)
  PONG_STACK_MANAGER_HEALTH_INTERVAL  Seconds between health polls (default: 2)
USAGE
}

if [[ $# -lt 1 ]]; then
  usage
  exit 1
fi

SETUP_BEACH=false
CREATE_BEACH=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --setup-beach)
      SETUP_BEACH=true
      shift
      ;;
    --create-beach)
      CREATE_BEACH=true
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    --)
      shift
      break
      ;;
    -*)
      echo "unknown option: $1" >&2
      usage
      exit 1
      ;;
    *)
      break
      ;;
  esac
done

if [[ $# -lt 1 ]]; then
  usage
  exit 1
fi

COMMAND=$1
shift

SERVICE=${PONG_DOCKER_SERVICE:-beach-manager}
MANAGER_URL=${PRIVATE_BEACH_MANAGER_URL:-http://localhost:8080}
SESSION_SERVER=${PONG_SESSION_SERVER:-http://beach-road:4132/}
AUTH_GATEWAY=${PONG_AUTH_GATEWAY:-http://beach-gate:4133}
CLI_PROFILE=${PONG_BEACH_PROFILE:-local}
LOG_ROOT=${PONG_LOG_ROOT:-/tmp/pong-stack}
LOG_DIR=${PONG_LOG_DIR:-$LOG_ROOT}
CODES_WAIT=${PONG_CODES_WAIT:-8}
ROLE_BOOTSTRAP_TIMEOUT=${PONG_ROLE_BOOTSTRAP_TIMEOUT:-40}
ROLE_BOOTSTRAP_INTERVAL=${PONG_ROLE_BOOTSTRAP_INTERVAL:-1}
REPO_ROOT=/app
CARGO_BIN_DIR=${PONG_CARGO_BIN_DIR:-/usr/local/cargo/bin}
PLAYER_LOG_LEVEL=${PONG_LOG_LEVEL:-info}
AGENT_LOG_LEVEL=${PONG_AGENT_LOG_LEVEL:-$PLAYER_LOG_LEVEL}
PREBUILD_BEACH_BIN=${PONG_STACK_PREBUILD:-1}
CONTAINER_ICE_IP=${PONG_STACK_CONTAINER_ICE_IP:-}
CONTAINER_ICE_HOST=${PONG_STACK_CONTAINER_ICE_HOST:-}
PONG_FRAME_DUMP_DIR=${PONG_FRAME_DUMP_DIR:-$LOG_DIR/frame-dumps}
PONG_BALL_TRACE_DIR=${PONG_BALL_TRACE_DIR:-$LOG_DIR/ball-trace}
PONG_COMMAND_TRACE_DIR=${PONG_COMMAND_TRACE_DIR:-$LOG_DIR/command-trace}
CONTAINER_ENV_PREFIX=""
# Preserve NAT hints by default so hosts advertise a reachable address; allow explicit overrides.
if [[ -n "$CONTAINER_ICE_IP" ]]; then
  CONTAINER_ENV_PREFIX+=" export BEACH_ICE_PUBLIC_IP='$CONTAINER_ICE_IP';"
fi
if [[ -n "$CONTAINER_ICE_HOST" ]]; then
  CONTAINER_ENV_PREFIX+=" export BEACH_ICE_PUBLIC_HOST='$CONTAINER_ICE_HOST';"
fi
HOST_MANAGER_TOKEN=${HOST_MANAGER_TOKEN:-""}
HOST_TOKEN_EXPORT=${HOST_TOKEN_EXPORT:-""}
STACK_MANAGER_TOKEN=${STACK_MANAGER_TOKEN:-""}
API_RESPONSE=""
API_STATUS=""

resolve_host_token() {
  local profile="$1"
  python3 - "$profile" <<'PY'
import os, sys
profile = sys.argv[1]
path = os.path.expanduser('~/.beach/credentials')
target_section = f"profiles.{profile}.access_token"
section = None
token = None
try:
    with open(path, 'r', encoding='utf-8') as fh:
        for raw in fh:
            line = raw.strip()
            if not line or line.startswith('#'):
                continue
            if line.startswith('[') and line.endswith(']'):
                section = line[1:-1].strip()
                continue
            if section == target_section and line.startswith('token'):
                try:
                    _, value = line.split('=', 1)
                    token = value.strip().strip('"')
                    break
                except ValueError:
                    pass
except FileNotFoundError:
    pass
if not token:
    sys.exit(1)
sys.stdout.write(token)
PY
}

if [[ -z "${PONG_DISABLE_HOST_TOKEN:-}" ]]; then
  if HOST_MANAGER_TOKEN=$(resolve_host_token "$CLI_PROFILE" 2>/dev/null); then
    if [[ -n "$HOST_MANAGER_TOKEN" ]]; then
      export HOST_MANAGER_TOKEN
      HOST_TOKEN_EXPORT="export PB_MANAGER_TOKEN='$HOST_MANAGER_TOKEN'; export PB_MCP_TOKEN='$HOST_MANAGER_TOKEN'; export PB_CONTROLLER_TOKEN='$HOST_MANAGER_TOKEN'; "
      echo "[pong-stack] using host Beach Auth token for profile '$CLI_PROFILE'" >&2
    fi
  else
    HOST_MANAGER_TOKEN=""
  fi
fi

HOST_ACCOUNT_ID=""
HOST_ACCOUNT_SUBJECT=""
HOST_ACCOUNT_EMAIL=""

if [[ -n "$HOST_MANAGER_TOKEN" ]]; then
  account_info=$(python3 - <<'PY'
import os, sys, json, base64
token = os.environ.get('HOST_MANAGER_TOKEN')
if not token:
    sys.exit(1)
parts = token.split('.')
if len(parts) != 3:
    sys.exit(1)
padding = '=' * (-len(parts[1]) % 4)
try:
    payload = json.loads(base64.urlsafe_b64decode(parts[1] + padding).decode('utf-8'))
except Exception:
    sys.exit(1)
account_id = payload.get('account_id') or payload.get('sub')
subject = payload.get('sub') or ''
email = payload.get('email') or 'host-user@beach.test'
if not account_id:
    sys.exit(1)
print(f"{account_id}::{subject}::{email}")
PY
  ) || account_info=""
  if [[ -n "$account_info" ]]; then
    IFS='::' read -r HOST_ACCOUNT_ID HOST_ACCOUNT_SUBJECT HOST_ACCOUNT_EMAIL <<<"$account_info"
  fi
fi

ensure_host_account() {
  if [[ -z "$HOST_ACCOUNT_ID" ]]; then
    return
  fi
  local subject="$HOST_ACCOUNT_SUBJECT"
  local email="$HOST_ACCOUNT_EMAIL"
  local display_name="Host User"
  docker compose exec -T postgres psql -U postgres -d beach_manager -c "INSERT INTO account (id, type, status, beach_gate_subject, display_name, email) VALUES ('$HOST_ACCOUNT_ID', 'user', 'active', '$subject', '$display_name', '$email') ON CONFLICT (id) DO UPDATE SET status='active', beach_gate_subject=EXCLUDED.beach_gate_subject, email=EXCLUDED.email" >/dev/null 2>&1 || true
}

run_in_container() {
  local cmd="$1"
  docker compose exec -T "$SERVICE" bash -c "$CONTAINER_ENV_PREFIX export PATH=\"$CARGO_BIN_DIR:\\$PATH\"; $cmd"
}

# Fresh per-run log directory: timestamped by default, with a stable "latest" symlink.
if [[ "$COMMAND" == start* ]]; then
  if [[ -z "${PONG_LOG_DIR:-}" ]]; then
    LOG_DIR="$LOG_ROOT/$(date +%Y%m%d-%H%M%S)"
  fi
  run_in_container "set -euo pipefail; mkdir -p '$LOG_ROOT'; rm -rf '$LOG_DIR'; mkdir -p '$LOG_DIR'; ln -sfn '$LOG_DIR' '$LOG_ROOT/latest'"
  # Recompute trace dirs after LOG_DIR is finalized.
  if [[ -z "${PONG_FRAME_DUMP_DIR+set}" ]]; then
    PONG_FRAME_DUMP_DIR="$LOG_DIR/frame-dumps"
  fi
  if [[ -z "${PONG_BALL_TRACE_DIR+set}" ]]; then
    PONG_BALL_TRACE_DIR="$LOG_DIR/ball-trace"
  fi
  if [[ -z "${PONG_COMMAND_TRACE_DIR+set}" ]]; then
    PONG_COMMAND_TRACE_DIR="$LOG_DIR/command-trace"
  fi
fi

prebuild_beach_binary() {
  if [[ "$PREBUILD_BEACH_BIN" -eq 0 ]]; then
    return
  fi
  echo "[pong-stack] ensuring beach binary is built inside $SERVICE..." >&2
  run_in_container "set -euo pipefail; cd $REPO_ROOT; cargo run --bin beach -- --version >/tmp/pong-stack-build.log 2>&1" || {
    echo "[pong-stack] cargo run failed; inspect /tmp/pong-stack-build.log inside $SERVICE" >&2
    exit 1
  }
}

service_running() {
  local state
  state=$(docker compose ps --format '{{.Name}} {{.State}}' "$SERVICE" 2>/dev/null | awk '{print $2}' | head -n1)
  [[ "$state" == "running" ]]
}

ensure_service_running() {
  if ! service_running; then
    echo "[pong-stack] docker compose service '$SERVICE' is not running; start the stack before invoking pong-stack.sh" >&2
    exit 1
  fi
}

ensure_cli_login() {
  if [[ -n "$HOST_MANAGER_TOKEN" ]]; then
    return
  fi
  run_in_container "set -euo pipefail; if [[ ! -s \$HOME/.beach/credentials ]]; then echo '[pong-stack] no beach CLI credentials found; launching beach login...' >&2; cd $REPO_ROOT; BEACH_AUTH_GATEWAY='$AUTH_GATEWAY' cargo run --bin beach -- login --name '$CLI_PROFILE' --force; fi"
}

start_player() {
  local mode=$1
  local human=$2
  local frame_env=""
  frame_env+="export PONG_VERBOSE_DIAG=1; "
  if [[ -n "${PONG_FRAME_DUMP_DIR:-}" ]]; then
    frame_env+="mkdir -p '$PONG_FRAME_DUMP_DIR'; export PONG_FRAME_DUMP_PATH='$PONG_FRAME_DUMP_DIR/frame-$mode.txt'; "
  fi
  if [[ -n "${PONG_FRAME_DUMP_INTERVAL:-}" ]]; then
    frame_env+="export PONG_FRAME_DUMP_INTERVAL='${PONG_FRAME_DUMP_INTERVAL}'; "
  fi
  if [[ -n "${PONG_BALL_TRACE_DIR:-}" ]]; then
    frame_env+="mkdir -p '$PONG_BALL_TRACE_DIR'; : > '$PONG_BALL_TRACE_DIR/ball-trace-$mode.jsonl'; export PONG_BALL_TRACE_PATH='$PONG_BALL_TRACE_DIR/ball-trace-$mode.jsonl'; "
  fi
  if [[ -n "${PONG_COMMAND_TRACE_DIR:-}" ]]; then
    frame_env+="mkdir -p '$PONG_COMMAND_TRACE_DIR'; : > '$PONG_COMMAND_TRACE_DIR/command-$mode.log'; export PONG_COMMAND_TRACE_PATH='$PONG_COMMAND_TRACE_DIR/command-$mode.log'; "
  fi
  run_in_container "set -euo pipefail; export PRIVATE_BEACH_MANAGER_URL='$MANAGER_URL'; export BEACH_AUTH_GATEWAY='$AUTH_GATEWAY'; ${frame_env}cd $REPO_ROOT; mkdir -p '$LOG_DIR'; : > '$LOG_DIR/beach-host-$mode.log'; nohup setsid bash -c \"cargo run --bin beach -- --log-level '$PLAYER_LOG_LEVEL' --log-file '$LOG_DIR/beach-host-$mode.log' --session-server '$SESSION_SERVER' host --bootstrap-output json --wait -- /usr/bin/env python3 $REPO_ROOT/apps/private-beach/demo/pong/player/main.py --mode $mode 2>&1 | tee '$LOG_DIR/bootstrap-$mode.json' > '$LOG_DIR/player-$mode.log'\" >/dev/null 2>&1 & echo \$! > '$LOG_DIR/player-$mode.pid'"
  echo "launched $human player (logs in $LOG_DIR/player-$mode.log)"
}

start_agent() {
  local beach_id=$1
  if [[ -n "$HOST_MANAGER_TOKEN" ]]; then
    ensure_host_account
  fi
  run_in_container "set -euo pipefail; export PRIVATE_BEACH_MANAGER_URL='$MANAGER_URL'; export PRIVATE_BEACH_ID='$beach_id'; export RUN_AGENT_SESSION_SERVER='$SESSION_SERVER'; export BEACH_AUTH_GATEWAY='$AUTH_GATEWAY'; ${HOST_TOKEN_EXPORT}export LOG_DIR='$LOG_DIR'; export PONG_AGENT_LOG_LEVEL='$AGENT_LOG_LEVEL'; export PONG_WATCHDOG_INTERVAL='${PONG_WATCHDOG_INTERVAL:-}'; cd $REPO_ROOT; mkdir -p '$LOG_DIR'; : > '$LOG_DIR/agent.log'; chmod +x apps/private-beach/demo/pong/tools/run-agent.sh; nohup setsid apps/private-beach/demo/pong/tools/run-agent.sh '$beach_id' > '$LOG_DIR/agent.log' 2>&1 & echo \$! > '$LOG_DIR/agent.pid'"
  echo "launched pong agent (logs in $LOG_DIR/agent.log)"
}

role_pid_path() {
  local role=$1
  if [[ "$role" == "agent" ]]; then
    printf '%s\n' "$LOG_DIR/agent.pid"
  else
    printf '%s\n' "$LOG_DIR/player-$role.pid"
  fi
}

role_primary_log_path() {
  local role=$1
  if [[ "$role" == "agent" ]]; then
    printf '%s\n' "$LOG_DIR/agent.log"
  else
    printf '%s\n' "$LOG_DIR/player-$role.log"
  fi
}

role_host_log_path() {
  local role=$1
  if [[ "$role" == "agent" ]]; then
    printf '%s\n' "$LOG_DIR/beach-host-agent.log"
  else
    printf '%s\n' "$LOG_DIR/beach-host-$role.log"
  fi
}

check_role_bootstrap() {
  local role=$1
  local py_script
  read -r -d '' py_script <<'PY'
import json
import os
import sys

role = os.environ["ROLE"]
log_dir = os.environ.get("LOG_DIR", "/tmp/pong-stack")
path = os.path.join(log_dir, f"bootstrap-{role}.json")
try:
    with open(path, "r", encoding="utf-8") as fh:
        for raw in fh:
            line = raw.strip()
            if not line:
                continue
            try:
                payload = json.loads(line)
            except json.JSONDecodeError:
                continue
            if payload.get("schema") != 2:
                continue
            session = payload.get("session_id") or payload.get("sessionId")
            code = (
                payload.get("join_code")
                or payload.get("verify_code")
                or payload.get("code")
                or payload.get("passcode")
            )
            if session and code:
                print(f"{session} {code}")
                sys.exit(0)
except OSError:
    pass
sys.exit(1)
PY
  local cmd
  printf -v cmd $'set -euo pipefail; export LOG_DIR=%q; export ROLE=%q; python3 - <<\'PY\'\n%s\nPY\n' "$LOG_DIR" "$role" "$py_script"
  run_in_container "$cmd"
}

role_process_alive() {
  local role=$1
  local pid_file
  pid_file=$(role_pid_path "$role")
  local cmd
  printf -v cmd $'set -euo pipefail; pid_file=%q; if [[ ! -s "$pid_file" ]]; then exit 0; fi; pid=$(cat "$pid_file" 2>/dev/null || true); if [[ -z "$pid" ]]; then exit 0; fi; if kill -0 "$pid" 2>/dev/null; then exit 0; fi; exit 1\n' "$pid_file"
  if run_in_container "$cmd"; then
    return 0
  fi
  return 1
}

debug_role_bootstrap() {
  local role=$1
  local human=$2
  local primary_log host_log bootstrap_file
  primary_log=$(role_primary_log_path "$role")
  host_log=$(role_host_log_path "$role")
  bootstrap_file="$LOG_DIR/bootstrap-$role.json"
  local cmd
  printf -v cmd $'set -euo pipefail;\nprimary=%q;\nhost=%q;\nbootstrap=%q;\nlabel=%q;\nshow_file() {\n  local desc=$1\n  local file=$2\n  echo "[pong-stack] ${label} ${desc}:"\n  if [[ -f "$file" ]]; then\n    tail -n 80 "$file" || true\n  else\n    echo "  (missing $file)"\n  fi\n}\nshow_file "process log ($primary)" "$primary"\nshow_file "host log ($host)" "$host"\nif [[ -f "$bootstrap" ]]; then\n  echo "[pong-stack] ${label} bootstrap payload ($bootstrap):"\n  cat "$bootstrap"\nelse\n  echo "[pong-stack] ${label} bootstrap payload missing ($bootstrap)"\nfi\n' "$primary_log" "$host_log" "$bootstrap_file" "$human"
  run_in_container "$cmd" || true
}

wait_for_role_bootstrap() {
  local role=$1
  local human=$2
  local timeout=${3:-$ROLE_BOOTSTRAP_TIMEOUT}
  local start_ts
  start_ts=$(date +%s)
  local session_info=""
  while true; do
    if session_info=$(check_role_bootstrap "$role" 2>/dev/null); then
      local session code
      read -r session code <<<"$session_info"
      echo "[pong-stack] $human bootstrap ready session_id=$session code=$code"
      return 0
    fi
    if ! role_process_alive "$role"; then
      echo "[pong-stack] $human process exited before bootstrap was observed."
      debug_role_bootstrap "$role" "$human"
      return 1
    fi
    local now elapsed
    now=$(date +%s)
    elapsed=$((now - start_ts))
    if [[ $elapsed -ge $timeout ]]; then
      echo "[pong-stack] $human bootstrap not observed within ${timeout}s."
      debug_role_bootstrap "$role" "$human"
      return 1
    fi
    sleep "$ROLE_BOOTSTRAP_INTERVAL"
  done
}

print_codes() {
  run_in_container "set -euo pipefail; cd $REPO_ROOT; LOG_DIR='$LOG_DIR' apps/private-beach/demo/pong/tools/print-session-codes.sh"
}

stop_stack() {
  run_in_container "pkill -f '[p]ong/player/main.py --mode lhs' 2>/dev/null || true; pkill -f '[p]ong/player/main.py --mode rhs' 2>/dev/null || true; pkill -f '[p]ong_showcase' 2>/dev/null || true; pkill -f '[r]un-agent.sh' 2>/dev/null || true; rm -f '$LOG_DIR'/player-*.pid '$LOG_DIR'/agent.pid"
  echo "terminated pong demo processes in $SERVICE"
}

resolve_container_manager_token() {
  local py_script cmd
  read -r -d '' py_script <<'PY'
import os
import sys

profile = os.environ.get("CLI_PROFILE") or "local"
path = os.path.expanduser("~/.beach/credentials")
target_section = f"profiles.{profile}.access_token"
section = None
token = None
try:
    with open(path, "r", encoding="utf-8") as fh:
        for raw in fh:
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            if line.startswith("[") and line.endswith("]"):
                section = line[1:-1].strip()
                continue
            if section == target_section and line.startswith("token"):
                parts = line.split("=", 1)
                if len(parts) == 2:
                    candidate = parts[1].strip().strip('"')
                    if candidate:
                        token = candidate
                        break
except FileNotFoundError:
    pass
if not token:
    sys.exit(1)
print(token)
PY
  printf -v cmd $'set -euo pipefail;\nexport CLI_PROFILE=%q;\npython3 - <<\'PY\'\n%s\nPY\n' "$CLI_PROFILE" "$py_script"
  run_in_container "$cmd"
}

get_stack_manager_token() {
  if [[ -n "$STACK_MANAGER_TOKEN" ]]; then
    printf '%s\n' "$STACK_MANAGER_TOKEN"
    return 0
  fi
  if [[ -n "$HOST_MANAGER_TOKEN" ]]; then
    STACK_MANAGER_TOKEN="$HOST_MANAGER_TOKEN"
    printf '%s\n' "$STACK_MANAGER_TOKEN"
    return 0
  fi
  ensure_cli_login
  local container_token
  if ! container_token=$(resolve_container_manager_token); then
    echo "[pong-stack] unable to resolve manager token inside container" >&2
    return 1
  fi
  STACK_MANAGER_TOKEN="$container_token"
  printf '%s\n' "$STACK_MANAGER_TOKEN"
}

create_private_beach() {
  local name slug payload_file id
  name=${PONG_BEACH_NAME:-"Pong Showcase"}
  slug=${PONG_BEACH_SLUG:-}
  payload_file=$(mktemp 2>/dev/null || mktemp -t pong-beach-payload)
  python3 - "$name" "$slug" >"$payload_file" <<'PY'
import json, sys
name = sys.argv[1]
slug = sys.argv[2] if len(sys.argv) > 2 else ""
payload = {"name": name}
if slug.strip():
    payload["slug"] = slug.strip()
print(json.dumps(payload))
PY
  if ! manager_api_request "POST" "/private-beaches" "$payload_file"; then
    rm -f "$payload_file"
    return 1
  fi
  rm -f "$payload_file"
  id=$(API_RESPONSE="$API_RESPONSE" python3 - <<'PY'
import json
import os

raw = os.environ.get("API_RESPONSE", "")
try:
    data = json.loads(raw)
except Exception:
    data = {}
beach_id = data.get("id") or ""
print(beach_id)
PY
)
  if [[ -z "$id" ]]; then
    echo "[pong-stack] failed to parse beach id from create response (status ${API_STATUS:-unknown}): $API_RESPONSE" >&2
    return 1
  fi
  echo "[pong-stack] created private beach $id" >&2
  if [[ -n "${PONG_CREATED_BEACH_ID_FILE:-}" ]]; then
    mkdir -p "$(dirname "$PONG_CREATED_BEACH_ID_FILE")"
    printf '%s\n' "$id" >"$PONG_CREATED_BEACH_ID_FILE"
  fi
  printf '%s\n' "$id"
}

manager_api_request() {
  local method=$1
  local path=$2
  local body_file=${3:-}
  local token
  if ! token=$(get_stack_manager_token); then
    return 1
  fi
  local base="${MANAGER_URL%/}"
  local url="${base}${path}"
  local response_file http_code
  response_file=$(mktemp 2>/dev/null || mktemp -t pong-stack-resp)
  local -a curl_args
  curl_args=(-sS -o "$response_file" -w '%{http_code}' -X "$method" -H "authorization: Bearer $token")
  if [[ -n "$body_file" ]]; then
    curl_args+=(-H "content-type: application/json" "--data-binary" "@$body_file")
  fi
  local attempt=0
  while [[ $attempt -lt 5 ]]; do
    attempt=$((attempt + 1))
    http_code=$(curl "${curl_args[@]}" "$url") || http_code=""
    if [[ -z "$http_code" ]]; then
      sleep 1
      continue
    fi
    if [[ "$http_code" == "401" || "$http_code" == "403" ]]; then
      if [[ -z "$HOST_MANAGER_TOKEN" ]]; then
        echo "[pong-stack] manager API ${method} ${path} returned ${http_code}; refreshing CLI credentials..." >&2
        STACK_MANAGER_TOKEN=""
        ensure_cli_login
        if ! token=$(get_stack_manager_token); then
          break
        fi
        curl_args=(-sS -o "$response_file" -w '%{http_code}' -X "$method" -H "authorization: Bearer $token")
        if [[ -n "$body_file" ]]; then
          curl_args+=(-H "content-type: application/json" "--data-binary" "@$body_file")
        fi
        continue
      fi
    fi
    break
  done
  if [[ -z "$http_code" ]]; then
    rm -f "$response_file"
    echo "[pong-stack] failed to reach manager at $url" >&2
    return 1
  fi
  API_RESPONSE=$(cat "$response_file")
  rm -f "$response_file"
  API_STATUS="$http_code"
  if [[ "$http_code" -ge 200 && "$http_code" -lt 300 ]]; then
    return 0
  fi
  echo "[pong-stack] manager API ${method} ${path} failed (status ${http_code})" >&2
  return 1
}

wait_for_manager() {
  local attempt=0
  local max_attempts=${PONG_STACK_MANAGER_HEALTH_ATTEMPTS:-30}
  local interval=${PONG_STACK_MANAGER_HEALTH_INTERVAL:-2}
  local url="${MANAGER_URL%/}/healthz"
  while [[ $attempt -lt $max_attempts ]]; do
    attempt=$((attempt + 1))
    if curl -fsS -o /dev/null "$url"; then
      return 0
    fi
    sleep "$interval"
  done
  return 1
}

run_showcase_preflight() {
  local beach_id="$1"
  if [[ -z "$beach_id" ]]; then
    return 0
  fi
  if ! manager_api_request "GET" "/private-beaches/$beach_id/showcase-preflight?refresh=1"; then
    echo "[pong-stack] unable to fetch showcase preflight diagnostics; continuing without preflight" >&2
    return 0
  fi
  if API_RESPONSE="$API_RESPONSE" python3 - <<'PY'; then
import json
import os
import sys

raw = os.environ.get("API_RESPONSE", "")
if not raw.strip():
    print("[pong-stack] showcase preflight returned empty payload; continuing.")
    sys.exit(0)

try:
    payload = json.loads(raw)
except Exception as exc:
    print(f"[pong-stack] invalid showcase preflight response: {exc}", file=sys.stderr)
    sys.exit(1)

status = str(payload.get("status") or "unknown").lower()
issues = payload.get("issues") or []
print(f"[pong-stack] showcase preflight status: {status}")
if not issues:
    print("[pong-stack] no blocking issues detected.")
allowed_codes = {
    "tile_missing",
    "agent_missing",
    "pairing_missing",
    "session_missing",
    "session_invalid",
    "tile_missing_session",
}
blocking = []
for issue in issues:
    severity = str(issue.get("severity") or "error")
    code = str(issue.get("code") or "issue")
    detail = str(issue.get("detail") or "")
    print(f"  - [{severity}] {code}: {detail}")
    remediation = issue.get("remediation")
    if isinstance(remediation, str) and remediation.strip():
        print(f"    fix: {remediation.strip()}")
    if severity == "error" and code not in allowed_codes:
        blocking.append(code)

if status == "blocked" and not blocking:
    print("[pong-stack] layout prerequisites missing; continuing with automatic seeding.")
    sys.exit(0)

sys.exit(2 if status == "blocked" else 0)
PY
    return 0
  else
    local status=$?
    if [[ $status -eq 2 ]]; then
      echo "[pong-stack] showcase preflight reported blocking issues; aborting launch" >&2
    else
      echo "[pong-stack] unable to parse showcase preflight response" >&2
    fi
    return "$status"
  fi
}

collect_session_bootstrap() {
  local py_script cmd output attempt
  read -r -d '' py_script <<'PY'
import json
import os
import sys

roles = ["lhs", "rhs", "agent"]
log_dir = os.environ.get("LOG_DIR", "/tmp/pong-stack")

def iter_json_objects(path: str):
    try:
        with open(path, "r", encoding="utf-8") as fh:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                try:
                    yield json.loads(line)
                except json.JSONDecodeError:
                    continue
    except OSError:
        return

def find_bootstrap(payloads):
    for payload in payloads:
        if not isinstance(payload, dict):
            continue
        if payload.get("schema") != 2:
            continue
        session = payload.get("session_id") or payload.get("sessionId")
        code = payload.get("join_code") or payload.get("verify_code") or payload.get("code") or payload.get("passcode")
        if session and code:
            return str(session).strip(), str(code).strip()
    return None, None

result = {}
missing = []
for role in roles:
    path = os.path.join(log_dir, f"bootstrap-{role}.json")
    session, code = find_bootstrap(iter_json_objects(path))
    if not session or not code:
        missing.append(role)
        continue
    result[role] = {"session_id": session, "code": code}
if missing:
    raise SystemExit("missing bootstrap data for: " + ", ".join(missing))
print(json.dumps(result))
PY
  printf -v cmd $'set -euo pipefail;\nexport LOG_DIR=%q;\npython3 - <<\'PY\'\n%s\nPY\n' "$LOG_DIR" "$py_script"
  for ((attempt = 1; attempt <= 5; attempt++)); do
    if output=$(run_in_container "$cmd"); then
      printf '%s\n' "$output"
      return 0
    fi
    sleep 1
  done
  echo "[pong-stack] unable to collect session bootstrap metadata from $LOG_DIR" >&2
  return 1
}

build_session_graph_payload() {
  local sessions_file="$1"
  python3 - "$sessions_file" <<'PY'
import json
import sys

sessions_path = sys.argv[1]
with open(sessions_path, "r", encoding="utf-8") as fh:
    sessions = json.load(fh)

required_roles = ["lhs", "rhs", "agent"]
for role in required_roles:
    if role not in sessions:
        raise SystemExit(f"missing session data for {role}")
    entry = sessions[role]
    if not entry.get("session_id") or not entry.get("code"):
        raise SystemExit(f"incomplete bootstrap data for {role}")

def normalize(role: str):
    entry = sessions[role]
    return str(entry["session_id"]).strip(), str(entry["code"]).strip()

lhs_session, lhs_code = normalize("lhs")
rhs_session, rhs_code = normalize("rhs")
agent_session, agent_code = normalize("agent")

tile_specs = {
    "lhs": {
        "tile_id": "pong-lhs",
        "node_type": "application",
        "title": "Pong LHS",
        "position": {"x": -540.0, "y": -40.0},
        "size": {"width": 440.0, "height": 320.0},
        "z_index": 1,
    },
    "rhs": {
        "tile_id": "pong-rhs",
        "node_type": "application",
        "title": "Pong RHS",
        "position": {"x": 200.0, "y": -40.0},
        "size": {"width": 440.0, "height": 320.0},
        "z_index": 2,
    },
    "agent": {
        "tile_id": "pong-agent",
        "node_type": "agent",
        "title": "Pong Agent",
        "position": {"x": -170.0, "y": 360.0},
        "size": {"width": 520.0, "height": 360.0},
        "z_index": 3,
    },
}

role_session_map = {
    "lhs": (lhs_session, lhs_code),
    "rhs": (rhs_session, rhs_code),
    "agent": (agent_session, agent_code),
}

tiles = []
for role, spec in tile_specs.items():
    session_id, code = role_session_map[role]
    entry = {
        "id": spec["tile_id"],
        "nodeType": spec["node_type"],
        "position": spec["position"],
        "size": spec["size"],
        "zIndex": spec["z_index"],
        "session": {
            "sessionId": session_id,
            "code": code,
            "title": spec["title"],
        },
    }
    if role == "agent":
        entry["agent"] = {
            "role": "Pong Agent",
            "responsibility": "Coordinate paddle movement for the Pong showcase.",
            "trace": {"enabled": True, "traceId": "pong-auto-setup"},
        }
    tiles.append(entry)

relationships = [
    {
        "id": "agent-to-lhs",
        "sourceId": "pong-agent",
        "targetId": "pong-lhs",
        "instructions": "Drive the left paddle to keep the volley alive.",
        "updateMode": "poll",
        "pollFrequency": 1,
        "promptTemplate": "Focus on keeping the ball away from the left wall.",
        "updateCadence": "slow",
    },
    {
        "id": "agent-to-rhs",
        "sourceId": "pong-agent",
        "targetId": "pong-rhs",
        "instructions": "Mirror positioning to cover the right side.",
        "updateMode": "poll",
        "pollFrequency": 1,
        "promptTemplate": "Maintain control of the right paddle and track the ball closely.",
        "updateCadence": "slow",
    },
]

body = {
    "clearExisting": True,
    "viewport": {"zoom": 0.9, "pan": {"x": -80.0, "y": 40.0}},
    "tiles": tiles,
    "relationships": relationships,
}

print(json.dumps(body))
PY
}

validate_session_graph_response() {
  local response_file="$1"
  local sessions_file="$2"
  python3 - "$response_file" "$sessions_file" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as fh:
    response = json.load(fh)
with open(sys.argv[2], "r", encoding="utf-8") as fh:
    sessions = json.load(fh)

tile_lookup = {"lhs": "pong-lhs", "rhs": "pong-rhs", "agent": "pong-agent"}
attachments = response.get("attachments") or []
attached_ids = {entry.get("tile_id") for entry in attachments if isinstance(entry, dict)}
missing = [tid for tid in tile_lookup.values() if tid not in attached_ids]
if missing:
    print("missing attachments for tiles: " + ", ".join(missing), file=sys.stderr)
    sys.exit(1)

pairings = response.get("pairings") or []
if len(pairings) < 2:
    print("expected at least two controller pairings", file=sys.stderr)
    sys.exit(1)

agent_session = sessions["agent"]["session_id"]
expected_children = {sessions["lhs"]["session_id"], sessions["rhs"]["session_id"]}
seen = set()
for pairing in pairings:
    if pairing.get("controller_session_id") != agent_session:
        continue
    child = pairing.get("child_session_id")
    if child in expected_children:
        seen.add(child)
if seen != expected_children:
    print("pairings missing child sessions: " + ", ".join(sorted(expected_children - seen)), file=sys.stderr)
    sys.exit(1)
PY
}

verify_sessions_attached() {
  local sessions_response_file="$1"
  local sessions_file="$2"
  python3 - "$sessions_response_file" "$sessions_file" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as fh:
    payload = json.load(fh)
if not isinstance(payload, list):
    print("unexpected sessions payload format", file=sys.stderr)
    sys.exit(1)
with open(sys.argv[2], "r", encoding="utf-8") as fh:
    sessions = json.load(fh)

expected = {str(entry["session_id"]) for entry in sessions.values()}
found = {entry.get("session_id") for entry in payload if isinstance(entry, dict)}
missing = expected - found
if missing:
    print("sessions missing from manager: " + ", ".join(sorted(missing)), file=sys.stderr)
    sys.exit(1)
PY
}

setup_private_beach() {
  local beach_id="$1"
  if [[ -z "$beach_id" ]]; then
    echo "[pong-stack] --setup-beach requires a private beach id" >&2
    return 1
  fi
  if ! command -v curl >/dev/null 2>&1; then
    echo "[pong-stack] curl is required for --setup-beach" >&2
    return 1
  fi
  if ! command -v python3 >/dev/null 2>&1; then
    echo "[pong-stack] python3 is required for --setup-beach" >&2
    return 1
  fi
  echo "[pong-stack] configuring private beach $beach_id via session-graph API" >&2
  if ! wait_for_manager; then
    echo "[pong-stack] manager health endpoint unreachable; aborting setup" >&2
    return 1
  fi
  local sessions_json
  if ! sessions_json=$(collect_session_bootstrap); then
    return 1
  fi
  local tmp_dir
  tmp_dir=$(mktemp -d 2>/dev/null || mktemp -d -t pong-stack-setup)
  local sessions_file="$tmp_dir/sessions.json"
  local payload_file="$tmp_dir/session-graph.json"
  local response_file="$tmp_dir/graph-response.json"
  local session_list_file="$tmp_dir/session-list.json"
  printf '%s' "$sessions_json" >"$sessions_file"
  local payload
  if ! payload=$(build_session_graph_payload "$sessions_file"); then
    rm -rf "$tmp_dir"
    return 1
  fi
  printf '%s' "$payload" >"$payload_file"
  if ! manager_api_request "POST" "/private-beaches/$beach_id/session-graph" "$payload_file"; then
    rm -rf "$tmp_dir"
    return 1
  fi
  printf '%s' "$API_RESPONSE" >"$response_file"
  if ! validate_session_graph_response "$response_file" "$sessions_file"; then
    rm -rf "$tmp_dir"
    return 1
  fi
  echo "[pong-stack] installed layout + controller pairings for $beach_id" >&2
  if ! manager_api_request "GET" "/private-beaches/$beach_id/sessions"; then
    rm -rf "$tmp_dir"
    return 1
  fi
  printf '%s' "$API_RESPONSE" >"$session_list_file"
  if ! verify_sessions_attached "$session_list_file" "$sessions_file"; then
    rm -rf "$tmp_dir"
    return 1
  fi
  echo "[pong-stack] verified all sessions attached to $beach_id" >&2
  rm -rf "$tmp_dir"
}

case "$COMMAND" in
  start)
    ensure_service_running
    if ! wait_for_manager; then
      echo "[pong-stack] manager $MANAGER_URL did not become healthy; aborting start" >&2
      exit 1
    fi
    prebuild_beach_binary
    if [[ $# -lt 1 ]]; then
      if [[ "$CREATE_BEACH" == true ]]; then
        target="__auto_beach__"
      else
        echo "error: missing start target" >&2
        usage
        exit 1
      fi
    else
      target=$1
      shift || true
    fi
    while [[ "$target" == "--setup-beach" || "$target" == "--create-beach" ]]; do
      if [[ "$target" == "--setup-beach" ]]; then
        SETUP_BEACH=true
      fi
      if [[ "$target" == "--create-beach" ]]; then
        CREATE_BEACH=true
      fi
      if [[ $# -lt 1 ]]; then
        if [[ "$CREATE_BEACH" == true ]]; then
          target="__auto_beach__"
          break
        else
          echo "error: missing start target" >&2
          usage
          exit 1
        fi
      fi
      target=$1
      shift || true
    done
    # Accept "start -- create-beach" CLI form by treating the post-terminator token
    # as the start target/flag.
    while [[ "$target" == "--" ]]; do
      if [[ $# -lt 1 ]]; then
        if [[ "$CREATE_BEACH" == true ]]; then
          target="__auto_beach__"
          break
        fi
        echo "error: missing start target" >&2
        usage
        exit 1
      fi
      target=$1
      shift || true
    done
    # Allow "create-beach" as a target alias that implies --create-beach with no id.
    if [[ "$target" == "create-beach" ]]; then
      CREATE_BEACH=true
      if [[ $# -gt 0 ]]; then
        target=$1
        shift || true
      else
        target="__auto_beach__"
      fi
    fi
    case "$target" in
      lhs)
        if [[ -z "$HOST_MANAGER_TOKEN" ]]; then
          ensure_cli_login
        fi
        start_player lhs "LHS"
        if [[ "$SETUP_BEACH" == true ]]; then
          echo "[pong-stack] --setup-beach requires starting the full stack; ignoring flag for 'start lhs'" >&2
        fi
        ;;
      rhs)
        if [[ -z "$HOST_MANAGER_TOKEN" ]]; then
          ensure_cli_login
        fi
        start_player rhs "RHS"
        if [[ "$SETUP_BEACH" == true ]]; then
          echo "[pong-stack] --setup-beach requires starting the full stack; ignoring flag for 'start rhs'" >&2
        fi
        ;;
      agent)
        if [[ $# -lt 1 ]]; then
          echo "error: start agent requires <private-beach-id>" >&2
          exit 1
        fi
        beach_id=$1
        if [[ -z "$HOST_MANAGER_TOKEN" ]]; then
          ensure_cli_login
        else
          echo "[pong-stack] skipping beach login inside container (forwarding host token)" >&2
        fi
        start_agent "$beach_id"
        if [[ "$SETUP_BEACH" == true ]]; then
          echo "[pong-stack] --setup-beach requires starting lhs + rhs + agent together; ignoring flag for agent-only start" >&2
        fi
        ;;
      *)
        beach_id=$target
        if [[ "$beach_id" == "__auto_beach__" ]]; then
          beach_id=""
        fi
        if [[ -z "$beach_id" && "$CREATE_BEACH" == true ]]; then
          if ! beach_id=$(create_private_beach); then
            exit 1
          fi
        fi
        if [[ -z "$beach_id" ]]; then
          echo "error: missing private beach id; provide one or use --create-beach" >&2
          exit 1
        fi
        if [[ -z "$HOST_MANAGER_TOKEN" ]]; then
          ensure_cli_login
        else
          echo "[pong-stack] skipping beach login inside container (forwarding host token)" >&2
        fi
        if ! run_showcase_preflight "$beach_id"; then
          exit $?
        fi
        start_player lhs "LHS"
        start_player rhs "RHS"
        start_agent "$beach_id"
        echo "[pong-stack] waiting for session bootstrap data (timeout ${ROLE_BOOTSTRAP_TIMEOUT}s per role)..."
        if ! wait_for_role_bootstrap lhs "LHS player"; then
          stop_stack
          exit 1
        fi
        if ! wait_for_role_bootstrap rhs "RHS player"; then
          stop_stack
          exit 1
        fi
        if ! wait_for_role_bootstrap agent "Pong agent"; then
          stop_stack
          exit 1
        fi
        echo "waiting $CODES_WAIT seconds for sessions to register..."
        sleep "$CODES_WAIT"
        print_codes
        if [[ "$SETUP_BEACH" == true ]]; then
          setup_private_beach "$beach_id"
        fi
        ;;
    esac
    ;;
  stop)
    if ! service_running; then
      echo "[pong-stack] service '$SERVICE' is not running; nothing to stop." >&2
      exit 0
    fi
    stop_stack
    ;;
  codes)
    ensure_service_running
    print_codes
    ;;
  *)
    echo "unknown command: $COMMAND" >&2
    usage
    exit 1
    ;;
esac

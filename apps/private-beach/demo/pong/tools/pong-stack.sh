#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  pong-stack.sh start <private-beach-id>  Start LHS, RHS, and agent inside docker compose service.
  pong-stack.sh start lhs                 Start only the LHS player.
  pong-stack.sh start rhs                 Start only the RHS player.
  pong-stack.sh start agent <pb-id>       Start only the mock agent.
  pong-stack.sh stop                      Stop all pong demo processes running in the service.
  pong-stack.sh codes                     Print current session IDs and passcodes from logs.

Environment variables:
  PONG_DOCKER_SERVICE   Docker compose service name (default: beach-manager)
  PRIVATE_BEACH_MANAGER_URL  Manager URL inside the container (default: http://localhost:8080)
  PONG_SESSION_SERVER   Beach session server URL (default: http://localhost:4132/)
  PONG_LOG_DIR          Log directory inside the container (default: /tmp/pong-stack)
  PONG_CODES_WAIT       Seconds to wait before printing session codes (default: 8)
USAGE
}

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
LOG_DIR=${PONG_LOG_DIR:-/tmp/pong-stack}
CODES_WAIT=${PONG_CODES_WAIT:-8}
REPO_ROOT=/app
CARGO_BIN_DIR=${PONG_CARGO_BIN_DIR:-/usr/local/cargo/bin}
PLAYER_LOG_LEVEL=${PONG_LOG_LEVEL:-info}
AGENT_LOG_LEVEL=${PONG_AGENT_LOG_LEVEL:-$PLAYER_LOG_LEVEL}
HOST_MANAGER_TOKEN=""
HOST_TOKEN_EXPORT=""

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
  docker compose exec -T "$SERVICE" bash -c "export PATH=\"$CARGO_BIN_DIR:\\$PATH\"; $cmd"
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
  run_in_container "set -euo pipefail; export PRIVATE_BEACH_MANAGER_URL='$MANAGER_URL'; export RUN_AGENT_SESSION_SERVER='$SESSION_SERVER'; export BEACH_AUTH_GATEWAY='$AUTH_GATEWAY'; ${HOST_TOKEN_EXPORT}export LOG_DIR='$LOG_DIR'; export PONG_AGENT_LOG_LEVEL='$AGENT_LOG_LEVEL'; cd $REPO_ROOT; mkdir -p '$LOG_DIR'; : > '$LOG_DIR/agent.log'; chmod +x apps/private-beach/demo/pong/tools/run-agent.sh; nohup setsid apps/private-beach/demo/pong/tools/run-agent.sh '$beach_id' > '$LOG_DIR/agent.log' 2>&1 & echo \$! > '$LOG_DIR/agent.pid'"
  echo "launched pong agent (logs in $LOG_DIR/agent.log)"
}

print_codes() {
  run_in_container "set -euo pipefail; cd $REPO_ROOT; LOG_DIR='$LOG_DIR' apps/private-beach/demo/pong/tools/print-session-codes.sh"
}

stop_stack() {
  run_in_container "pkill -f '[p]ong/player/main.py --mode lhs' 2>/dev/null || true; pkill -f '[p]ong/player/main.py --mode rhs' 2>/dev/null || true; pkill -f '[p]ong_showcase' 2>/dev/null || true; pkill -f '[r]un-agent.sh' 2>/dev/null || true; rm -f '$LOG_DIR'/player-*.pid '$LOG_DIR'/agent.pid"
  echo "terminated pong demo processes in $SERVICE"
}

case "$COMMAND" in
  start)
    if [[ $# -lt 1 ]]; then
      echo "error: missing start target" >&2
      usage
      exit 1
    fi
    target=$1
    shift || true
    case "$target" in
      lhs)
        start_player lhs "LHS"
        ;;
      rhs)
        start_player rhs "RHS"
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
        ;;
      *)
        beach_id=$target
        if [[ -z "$HOST_MANAGER_TOKEN" ]]; then
          ensure_cli_login
        else
          echo "[pong-stack] skipping beach login inside container (forwarding host token)" >&2
        fi
        start_player lhs "LHS"
        start_player rhs "RHS"
        start_agent "$beach_id"
        echo "waiting $CODES_WAIT seconds for sessions to register..."
        sleep "$CODES_WAIT"
        print_codes
        ;;
    esac
    ;;
  stop)
    stop_stack
    ;;
  codes)
    print_codes
    ;;
  *)
    echo "unknown command: $COMMAND" >&2
    usage
    exit 1
    ;;
esac

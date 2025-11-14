#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  pong-stack.sh start <private-beach-id>  Start LHS, RHS, and agent inside docker compose service.
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
SESSION_SERVER=${PONG_SESSION_SERVER:-http://host.docker.internal:4132/}
AUTH_GATEWAY=${PONG_AUTH_GATEWAY:-http://host.docker.internal:4133}
CLI_PROFILE=${PONG_BEACH_PROFILE:-local}
LOG_DIR=${PONG_LOG_DIR:-/tmp/pong-stack}
CODES_WAIT=${PONG_CODES_WAIT:-8}
REPO_ROOT=/app
CARGO_BIN_DIR=${PONG_CARGO_BIN_DIR:-/usr/local/cargo/bin}

run_in_container() {
  local cmd="$1"
  docker compose exec -T "$SERVICE" bash -c "export PATH=\"$CARGO_BIN_DIR:\\$PATH\"; $cmd"
}

ensure_cli_login() {
  run_in_container "set -euo pipefail; if [[ ! -s \$HOME/.beach/credentials ]]; then echo '[pong-stack] no beach CLI credentials found; launching beach login...' >&2; cd $REPO_ROOT; BEACH_AUTH_GATEWAY='$AUTH_GATEWAY' cargo run --bin beach -- login --name '$CLI_PROFILE' --force; fi"
}

start_player() {
  local mode=$1
  local human=$2
  run_in_container "set -euo pipefail; export PRIVATE_BEACH_MANAGER_URL='$MANAGER_URL'; cd $REPO_ROOT; mkdir -p '$LOG_DIR'; nohup setsid bash -c \"cargo run --bin beach -- --log-level trace --log-file '$LOG_DIR/beach-host-$mode.log' --session-server '$SESSION_SERVER' host --bootstrap-output json --wait -- /usr/bin/env python3 $REPO_ROOT/apps/private-beach/demo/pong/player/main.py --mode $mode 2>&1 | tee '$LOG_DIR/bootstrap-$mode.json' > '$LOG_DIR/player-$mode.log'\" >/dev/null 2>&1 & echo \$! > '$LOG_DIR/player-$mode.pid'"
  echo "launched $human player (logs in $LOG_DIR/player-$mode.log)"
}

start_agent() {
  local beach_id=$1
  run_in_container "set -euo pipefail; export PRIVATE_BEACH_MANAGER_URL='$MANAGER_URL'; export RUN_AGENT_SESSION_SERVER='$SESSION_SERVER'; export BEACH_AUTH_GATEWAY='$AUTH_GATEWAY'; export LOG_DIR='$LOG_DIR'; cd $REPO_ROOT; mkdir -p '$LOG_DIR'; chmod +x apps/private-beach/demo/pong/tools/run-agent.sh; nohup setsid apps/private-beach/demo/pong/tools/run-agent.sh '$beach_id' > '$LOG_DIR/agent.log' 2>&1 & echo \$! > '$LOG_DIR/agent.pid'"
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
      echo "error: missing <private-beach-id>" >&2
      usage
      exit 1
    fi
    beach_id=$1
    ensure_cli_login
    start_player lhs "LHS"
    start_player rhs "RHS"
    start_agent "$beach_id"
    echo "waiting $CODES_WAIT seconds for sessions to register..."
    sleep "$CODES_WAIT"
    print_codes
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

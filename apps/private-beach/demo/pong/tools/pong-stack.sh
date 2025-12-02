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
  PONG_MANAGER_IMPL     legacy|rewrite (default: legacy) to select manager service/port
  PONG_DOCKER_SERVICE   Docker compose service name (default: beach-manager; auto-set if impl=rewrite)
  PRIVATE_BEACH_MANAGER_URL  Manager URL inside the container (default: http://localhost:8080; auto-set if impl=rewrite)
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

PONG_MANAGER_IMPL=${PONG_MANAGER_IMPL:-legacy}
if [[ "$PONG_MANAGER_IMPL" == "rewrite" ]]; then
  SERVICE=${PONG_DOCKER_SERVICE:-beach-manager-rewrite}
  MANAGER_URL=${PRIVATE_BEACH_MANAGER_URL:-http://localhost:8081}
else
  SERVICE=${PONG_DOCKER_SERVICE:-beach-manager}
  MANAGER_URL=${PRIVATE_BEACH_MANAGER_URL:-http://localhost:8080}
fi
SESSION_SERVER_BASE=${BEACH_SESSION_SERVER_BASE:-}
SESSION_SERVER=${SESSION_SERVER_BASE:-${PONG_SESSION_SERVER:-http://api.beach.dev:4132/}}
AUTH_GATEWAY=${PONG_AUTH_GATEWAY:-http://beach-gate:4133}
CLI_PROFILE=${PONG_BEACH_PROFILE:-local}
LOG_ROOT=${PONG_LOG_ROOT:-/tmp/pong-stack}
LOG_DIR=${PONG_LOG_DIR:-$LOG_ROOT}
CODES_WAIT=${PONG_CODES_WAIT:-8}
ROLE_BOOTSTRAP_TIMEOUT=${PONG_ROLE_BOOTSTRAP_TIMEOUT:-180}
ROLE_BOOTSTRAP_INTERVAL=${PONG_ROLE_BOOTSTRAP_INTERVAL:-1}
if [[ -z "${PONG_ROLE_BOOTSTRAP_TIMEOUT:-}" ]]; then
  ROLE_BOOTSTRAP_TIMEOUT=420
fi
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
PONG_STATE_TRACE_DIR=${PONG_STATE_TRACE_DIR:-$LOG_DIR/state-trace}
PONG_STATE_TRACE_INTERVAL=${PONG_STATE_TRACE_INTERVAL:-1}
# Resolve a host-side LAN IP once so we can inject a browser-reachable ICE hint
# into the container when none is explicitly provided.
if [[ -z "$CONTAINER_ICE_IP" ]]; then
  if LAN_IP=$(ipconfig getifaddr "$(route get default | awk '/interface:/{print $2}')" 2>/dev/null); then
    CONTAINER_ICE_IP="$LAN_IP"
  fi
fi
if [[ -z "$CONTAINER_ICE_HOST" ]]; then
  CONTAINER_ICE_HOST="$CONTAINER_ICE_IP"
fi
CONTAINER_ENV_PREFIX=""
# Preserve NAT hints by default so hosts advertise a reachable address; allow explicit overrides.
if [[ -n "$CONTAINER_ICE_IP" ]]; then
  CONTAINER_ENV_PREFIX+=" export BEACH_ICE_PUBLIC_IP='$CONTAINER_ICE_IP';"
fi
if [[ -n "$CONTAINER_ICE_HOST" ]]; then
  CONTAINER_ENV_PREFIX+=" export BEACH_ICE_PUBLIC_HOST='$CONTAINER_ICE_HOST';"
fi
# Force transport/debug logging to keep attach/bridge traces visible regardless of host env.
CONTAINER_ENV_PREFIX+=" export RUST_LOG='info,beach::transport::webrtc=debug,transport.extension=debug,controller.actions=debug,private_beach=debug,webrtc=trace,webrtc::ice_transport=trace,webrtc::peer_connection=trace';"

# Ensure BEACH_TURN_EXTERNAL_IP is exported for docker compose if not already set.
if [[ -z "${BEACH_TURN_EXTERNAL_IP:-}" ]]; then
  if [[ -n "${LAN_IP:-}" ]]; then
    export BEACH_TURN_EXTERNAL_IP="$LAN_IP"
  elif LAN_IP=$(ipconfig getifaddr "$(route get default | awk '/interface:/{print $2}')" 2>/dev/null); then
    export BEACH_TURN_EXTERNAL_IP="$LAN_IP"
  fi
fi
# Default insecure manager token for dev/bypass flows.
: "${DEV_MANAGER_INSECURE_TOKEN:=DEV-MANAGER-TOKEN}"
: "${PRIVATE_BEACH_BYPASS_AUTH:=1}"
HOST_MANAGER_TOKEN=${HOST_MANAGER_TOKEN:-""}
HOST_TOKEN_EXPORT=${HOST_TOKEN_EXPORT:-""}
STACK_MANAGER_TOKEN=${STACK_MANAGER_TOKEN:-""}
if [[ -n "${BEACH_TOKEN:-}" ]]; then
  CONTAINER_ENV_PREFIX+=" export BEACH_TOKEN='$BEACH_TOKEN';"
fi
# Ensure dev/bypass tokens are present for manager API calls even if CLI creds are absent.
if [[ -n "${DEV_ALLOW_INSECURE_MANAGER_TOKEN:-1}" ]]; then
  CONTAINER_ENV_PREFIX+=" export DEV_ALLOW_INSECURE_MANAGER_TOKEN='1';"
fi
if [[ -n "${DEV_MANAGER_INSECURE_TOKEN:-}" ]]; then
  CONTAINER_ENV_PREFIX+=" export DEV_MANAGER_INSECURE_TOKEN='${DEV_MANAGER_INSECURE_TOKEN}';"
fi
if [[ -n "${PRIVATE_BEACH_MANAGER_TOKEN:-}" ]]; then
  CONTAINER_ENV_PREFIX+=" export PRIVATE_BEACH_MANAGER_TOKEN='${PRIVATE_BEACH_MANAGER_TOKEN}';"
fi
if [[ -n "${PRIVATE_BEACH_BYPASS_AUTH:-1}" ]]; then
  CONTAINER_ENV_PREFIX+=" export PRIVATE_BEACH_BYPASS_AUTH='${PRIVATE_BEACH_BYPASS_AUTH:-1}';"
fi
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
  if [[ -n "${PONG_DEBUG_RUN_IN_CONTAINER:-}" ]]; then
    echo "[run_in_container] cmd begin" >&2
    echo "$cmd" >&2
    echo "[run_in_container] cmd end" >&2
  fi
  printf '%s\n' "$cmd" | docker compose exec -T "$SERVICE" bash -lc "${CONTAINER_ENV_PREFIX} export PATH=\"$CARGO_BIN_DIR:\\$PATH\"; cat > /tmp/pong-run.sh; bash /tmp/pong-run.sh"
}

# Fresh per-run log directory: timestamped by default, with a stable "latest" symlink.
if [[ "$COMMAND" == start* ]]; then
  if [[ -z "${PONG_LOG_DIR:-}" ]]; then
    LOG_DIR="$LOG_ROOT/$(date +%Y%m%d-%H%M%S)"
  fi
  run_in_container "set -euo pipefail; mkdir -p '$LOG_ROOT'; rm -rf '$LOG_DIR'; mkdir -p '$LOG_DIR'; ln -sfn '$LOG_DIR' '$LOG_ROOT/latest'"
  # Recompute trace dirs after LOG_DIR is finalized.
  if [[ -z "${PONG_FRAME_DUMP_DIR+x}" || "$PONG_FRAME_DUMP_DIR" == "$LOG_ROOT/frame-dumps" ]]; then
    PONG_FRAME_DUMP_DIR="$LOG_DIR/frame-dumps"
  fi
  if [[ -z "${PONG_BALL_TRACE_DIR+x}" || "$PONG_BALL_TRACE_DIR" == "$LOG_ROOT/ball-trace" ]]; then
    PONG_BALL_TRACE_DIR="$LOG_DIR/ball-trace"
  fi
  if [[ -z "${PONG_COMMAND_TRACE_DIR+x}" || "$PONG_COMMAND_TRACE_DIR" == "$LOG_ROOT/command-trace" ]]; then
    PONG_COMMAND_TRACE_DIR="$LOG_DIR/command-trace"
  fi
  if [[ -z "${PONG_STATE_TRACE_DIR+x}" || "$PONG_STATE_TRACE_DIR" == "$LOG_ROOT/state-trace" ]]; then
    PONG_STATE_TRACE_DIR="$LOG_DIR/state-trace"
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

get_forge_script() {
cat <<'EOF'
import os
import time
import json
import base64
import sys
from cryptography.hazmat.primitives import serialization, hashes
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.utils import decode_dss_signature

def base64url_encode(data):
    return base64.urlsafe_b64encode(data).rstrip(b'=')

repo_root = os.getcwd()
key_path = os.path.join(repo_root, "config/dev-secrets/beach-gate-ec256.pem")
kid_path = os.path.join(repo_root, "config/dev-secrets/beach-gate-signing.kid")

if not os.path.exists(key_path):
    print(f"Warning: Keys not found at {key_path}, skipping token injection", file=sys.stderr)
    sys.exit(0)

try:
    with open(key_path, "rb") as f:
        private_key = serialization.load_pem_private_key(f.read(), password=None)
    with open(kid_path, "r") as f:
        kid = f.read().strip()
except FileNotFoundError:
    print(f"Warning: Keys not found at {key_path}, skipping token injection", file=sys.stderr)
    sys.exit(0)

header = {
    "alg": "ES256",
    "kid": kid,
    "typ": "JWT"
}

now = int(time.time())
exp = now + 365 * 24 * 3600  # 1 year

payload = {
    "iss": "beach-gate",
    "sub": "00000000-0000-0000-0000-000000000001",
    "aud": "private-beach-manager",
    "exp": exp,
    "iat": now,
    "entitlements": ["private-beach:turn", "rescue:fallback", "pb:transport.turn", "pb:harness.publish"],
    "tier": "standard",
    "profile": "default",
    "email": "mock-user@beach.test",
    "account_id": "00000000-0000-0000-0000-000000000001",
    "scope": "rescue:fallback private-beach:turn pb:beaches.read pb:beaches.write pb:sessions.read pb:sessions.write pb:control.read pb:control.write pb:control.consume pb:agents.onboard pb:harness.publish"
}

header_json = json.dumps(header, separators=(',', ':')).encode('utf-8')
payload_json = json.dumps(payload, separators=(',', ':')).encode('utf-8')

header_b64 = base64url_encode(header_json)
payload_b64 = base64url_encode(payload_json)

signing_input = header_b64 + b'.' + payload_b64
signature = private_key.sign(signing_input, ec.ECDSA(hashes.SHA256()))

r, s = decode_dss_signature(signature)
r_bytes = r.to_bytes(32, byteorder='big')
s_bytes = s.to_bytes(32, byteorder='big')
raw_signature = r_bytes + s_bytes
signature_b64 = base64url_encode(raw_signature)

token = (header_b64 + b'.' + payload_b64 + b'.' + signature_b64).decode('utf-8')

gateway = os.environ.get("BEACH_AUTH_GATEWAY", "http://beach-gate:4133")
creds_path = os.path.expanduser("~/.beach/credentials")
os.makedirs(os.path.dirname(creds_path), exist_ok=True)

toml = f"""current_profile = "forged"

[profiles.forged]
issuer = "{gateway}"
audience = "private-beach-manager"
updated_at = "{time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}"
entitlements = ["private-beach:turn", "rescue:fallback", "pb:transport.turn", "pb:harness.publish"]

[profiles.forged.refresh]
kind = "plain"
token = "dummy-refresh-token"

[profiles.forged.access_token]
token = "{token}"
expires_at = "2030-01-01T00:00:00Z"
entitlements = ["private-beach:turn", "rescue:fallback", "pb:transport.turn", "pb:harness.publish"]
"""

print(toml)
EOF
}

inject_container_credentials() {
  local container_name="$1"
  
  # Ensure credentials file exists
  if [ ! -f ~/.beach/credentials ]; then
      echo "Error: ~/.beach/credentials not found. Credentials should be forged globally."
      exit 1
  fi

  echo "Injecting credentials into $container_name..."
  
  # Create .beach directory in container
  run_in_container "mkdir -p /root/.beach"
  
  # Copy credentials file from host to container
  docker cp ~/.beach/credentials "$container_name:/root/.beach/credentials"
  
  # Set permissions
  run_in_container "chmod 600 /root/.beach/credentials"
  
  # Read the token back to pass as env var
  BEACH_TOKEN=$(grep "token =" ~/.beach/credentials | grep -v "dummy-refresh-token" | head -n 1 | cut -d '"' -f 2)
  export BEACH_TOKEN
}

ensure_cli_login() {
  # We are injecting credentials directly, so we can skip CLI login check
  # But we need to ensure the container is running first
  echo "[pong-stack] skipping beach login inside container (forwarding host token)"
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
  if [[ -n "${PONG_STATE_TRACE_DIR:-}" ]]; then
    frame_env+="mkdir -p '$PONG_STATE_TRACE_DIR'; : > '$PONG_STATE_TRACE_DIR/state-$mode.jsonl'; export PONG_STATE_TRACE_PATH='$PONG_STATE_TRACE_DIR/state-$mode.jsonl'; "
  fi
  if [[ -n "${PONG_STATE_TRACE_INTERVAL:-}" ]]; then
    frame_env+="export PONG_STATE_TRACE_INTERVAL='${PONG_STATE_TRACE_INTERVAL}'; "
  fi
  # Use global HOST_MANAGER_TOKEN
  local host_token="$HOST_MANAGER_TOKEN"
  
  if [[ -z "$host_token" ]]; then
      echo "Error: HOST_MANAGER_TOKEN not set. Credentials should be forged globally." >&2
      exit 1
  fi

  cmd=$(cat <<EOF
set -euo pipefail
export PRIVATE_BEACH_MANAGER_URL='$MANAGER_URL'
export BEACH_SESSION_SERVER_BASE='$SESSION_SERVER'
export BEACH_SESSION_SERVER='$SESSION_SERVER'
export BEACH_ROAD_URL='$SESSION_SERVER'
export BEACH_PUBLIC_SESSION_SERVER='$SESSION_SERVER'
export BEACH_AUTH_GATEWAY='$AUTH_GATEWAY'
export LOG_DIR='$LOG_DIR'
export MODE='$mode'
export SESSION_SERVER='$SESSION_SERVER'
export REPO_ROOT='$REPO_ROOT'
export CARGO_RUN_RETRIES='${CARGO_RUN_RETRIES:-12}'
export CARGO_RUN_RETRY_DELAY='${CARGO_RUN_RETRY_DELAY:-5}'
${frame_env}
cd $REPO_ROOT
mkdir -p '$LOG_DIR'
: > '$LOG_DIR/beach-host-$mode.log'
echo 'DEBUG: id' >> '$LOG_DIR/beach-host-$mode.log'; id >> '$LOG_DIR/beach-host-$mode.log'
echo 'DEBUG: env' >> '$LOG_DIR/beach-host-$mode.log'; env >> '$LOG_DIR/beach-host-$mode.log'

# Pass token from host
unset PB_MANAGER_TOKEN PB_MCP_TOKEN PB_CONTROLLER_TOKEN BEACH_TOKEN
export BEACH_TOKEN='$host_token'
echo "DEBUG: BEACH_TOKEN=\$BEACH_TOKEN" >> '$LOG_DIR/beach-host-$mode.log'

# Unset potential conflicting auth variables (except BEACH_TOKEN)
unset HOST_MANAGER_TOKEN BEACH_ACCESS_TOKEN CLERK_SECRET_KEY
EOF
)

  cmd+=$'\n'"$(cat <<'EOF'
  nohup setsid bash -c 'attempt=0; rc=1; while true; do cargo run --bin beach -- --log-level "debug" --log-file "$LOG_DIR/beach-host-$MODE.log" --session-server "$SESSION_SERVER" host --bootstrap-output json --wait -- /usr/bin/env python3 "$REPO_ROOT/apps/private-beach/demo/pong/player/main.py" --mode "$MODE" 2>&1 | tee "$LOG_DIR/bootstrap-$MODE.json" > "$LOG_DIR/player-$MODE.log"; rc=${PIPESTATUS[0]}; if [[ $rc -eq 0 ]]; then break; fi; attempt=$((attempt+1)); if [[ $attempt -ge ${CARGO_RUN_RETRIES:-12} ]]; then echo "cargo run failed after retries; giving up" >&2; exit $rc; fi; echo "cargo run failed (rc=$rc); retrying after ${CARGO_RUN_RETRY_DELAY:-5}s (attempt $attempt)..."; sleep ${CARGO_RUN_RETRY_DELAY:-5}; done' >/dev/null 2>&1 & echo $! > "$LOG_DIR/player-$MODE.pid"
EOF
)"

  run_in_container "$cmd"
  echo "launched $human player (logs in $LOG_DIR/player-$mode.log)"
}

start_agent() {
  local beach_id=$1
  if [[ -n "$HOST_MANAGER_TOKEN" ]]; then
    ensure_host_account
  fi
  run_in_container "set -euo pipefail; export HOME='/root'; export BEACH_CREDENTIALS_FILE='/root/.beach/credentials'; export PRIVATE_BEACH_MANAGER_URL='$MANAGER_URL'; export PRIVATE_BEACH_ID='$beach_id'; export RUN_AGENT_SESSION_SERVER='$SESSION_SERVER'; export BEACH_SESSION_SERVER_BASE='${SESSION_SERVER}'; export BEACH_SESSION_SERVER='$SESSION_SERVER'; export BEACH_ROAD_URL='$SESSION_SERVER'; export BEACH_PUBLIC_SESSION_SERVER='$SESSION_SERVER'; export BEACH_AUTH_GATEWAY='$AUTH_GATEWAY'; unset PB_MANAGER_TOKEN PB_MCP_TOKEN PB_CONTROLLER_TOKEN BEACH_TOKEN; ${HOST_TOKEN_EXPORT} export PB_MANAGER_TOKEN='${HOST_MANAGER_TOKEN}'; export PB_MCP_TOKEN='${HOST_MANAGER_TOKEN}'; export PB_CONTROLLER_TOKEN='${HOST_MANAGER_TOKEN}'; export BEACH_TOKEN='${HOST_MANAGER_TOKEN}'; export LOG_DIR='$LOG_DIR'; export PONG_AGENT_LOG_LEVEL='$AGENT_LOG_LEVEL'; export PONG_WATCHDOG_INTERVAL='${PONG_WATCHDOG_INTERVAL:-}'; mkdir -p /root/.beach && chmod 700 /root/.beach && chown root:root /root/.beach/credentials || true; chmod 600 /root/.beach/credentials || true; cat /root/.beach/credentials >/dev/null; { echo \"ENV_DUMP_START\"; env | grep -E '^(PB_|BEACH_|PRIVATE_BEACH_|RUN_AGENT_|PONG_)'; echo \"ENV_DUMP_END\"; } >> '$LOG_DIR/agent.env'; echo \"PB_MANAGER_TOKEN length=\${#PB_MANAGER_TOKEN}\" >> '$LOG_DIR/agent.log'; cd $REPO_ROOT; mkdir -p '$LOG_DIR'; : > '$LOG_DIR/agent.log'; chmod +x apps/private-beach/demo/pong/tools/run-agent.sh; nohup setsid apps/private-beach/demo/pong/tools/run-agent.sh '$beach_id' > '$LOG_DIR/agent.log' 2>&1 & echo \$! > '$LOG_DIR/agent.pid'"
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

sessions_attached_to_manager() {
  local beach_id="$1"
  local tmp_dir
  tmp_dir=$(mktemp -d 2>/dev/null || mktemp -d -t pong-stack-sessions)
  local sessions_file="$tmp_dir/sessions.json"
  local session_list_file="$tmp_dir/session-list.json"
  local ok=1
  if ! sessions_json=$(collect_session_bootstrap 2>/dev/null); then
    ok=1
  else
    printf '%s' "$sessions_json" >"$sessions_file"
    if manager_api_request "GET" "/private-beaches/$beach_id/sessions"; then
      printf '%s' "$API_RESPONSE" >"$session_list_file"
      if verify_sessions_attached "$session_list_file" "$sessions_file"; then
        ok=0
      fi
    fi
  fi
  rm -rf "$tmp_dir"
  return "$ok"
}

layout_missing() {
  local beach_id=$1
  if ! manager_api_request "GET" "/private-beaches/$beach_id/layout"; then
    return 0
  fi
  # Treat non-200 or empty payload as missing.
  if [[ -z "${API_STATUS:-}" || "$API_STATUS" -lt 200 || "$API_STATUS" -ge 300 ]]; then
    return 0
  fi
  if [[ -z "${API_RESPONSE:-}" ]]; then
    return 0
  fi
  if API_RESPONSE="$API_RESPONSE" python3 - <<'PY'
import json
import os

raw = os.environ.get("API_RESPONSE", "")
try:
    payload = json.loads(raw)
except Exception:
    raise SystemExit(1)

required_tiles = {"pong-lhs", "pong-rhs", "pong-agent"}
tiles = payload.get("tiles") or payload.get("nodes") or {}
if isinstance(tiles, dict):
    present_tiles = set(tiles.keys())
elif isinstance(tiles, list):
    present_tiles = {entry.get("id") for entry in tiles if isinstance(entry, dict)}
else:
    present_tiles = set()

missing = required_tiles - {t for t in present_tiles if t}
if missing:
    raise SystemExit(1)

# Require at least two relationships/edges so we do not treat a bare layout as valid.
relationships = payload.get("relationships") or payload.get("edges")
if relationships is None:
    relationships = (
        payload.get("metadata", {}).get("agentRelationships", {}).values()
    )
if not isinstance(relationships, (list, tuple, set)) and not hasattr(relationships, "__iter__"):
    raise SystemExit(1)
if len(list(relationships)) < 2:
    raise SystemExit(1)

raise SystemExit(0)
PY
  then
    # Layout is present and contains the expected nodes + relationships.
    return 1
  fi
  # Missing or incomplete layout.
  return 0
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
  if [[ -n "$DEV_MANAGER_INSECURE_TOKEN" ]]; then
    STACK_MANAGER_TOKEN="$DEV_MANAGER_INSECURE_TOKEN"
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
    echo "[pong-stack] create beach failed. Response: $API_RESPONSE" >&2
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
  local max_attempts=${PONG_MANAGER_API_RETRIES:-12}
  local retry_delay=${PONG_MANAGER_API_RETRY_DELAY:-2}
  while [[ $attempt -lt $max_attempts ]]; do
    attempt=$((attempt + 1))
    http_code=$(curl "${curl_args[@]}" "$url") || http_code=""
    if [[ -z "$http_code" ]]; then
      sleep "$retry_delay"
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
    if [[ "$http_code" == "000" || "$http_code" == "" || "$http_code" =~ ^5 ]]; then
      if [[ $attempt -lt $max_attempts ]]; then
        sleep "$retry_delay"
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



wait_for_manager_ready() {
  local url="${MANAGER_URL%/}/healthz"
  local attempt=0
  local max_attempts=${PONG_STACK_MANAGER_HEALTH_ATTEMPTS:-240}
  local interval=${PONG_STACK_MANAGER_HEALTH_INTERVAL:-3}
  while [[ $attempt -lt $max_attempts ]]; do
    attempt=$((attempt + 1))
    if curl -fsS -o /dev/null "$url"; then
      echo "[pong-stack] manager healthy at $url" >&2
      return 0
    fi
    sleep "$interval"
  done
  echo "[pong-stack] manager $MANAGER_URL did not become healthy after $max_attempts attempts" >&2
  return 1
}

kill_conflicting_ice_ports() {
  # Best-effort: free any processes holding the ICE UDP range before starting the stack.
  local start_port=${BEACH_ICE_PORT_START:-64000}
  local end_port=${BEACH_ICE_PORT_END:-64100}
  if ! command -v lsof >/dev/null 2>&1; then
    echo "[pong-stack] lsof not found; skipping ICE port cleanup" >&2
    return
  fi
  echo "[pong-stack] checking for processes on UDP ports ${start_port}-${end_port}..." >&2
  local lsof_out
  lsof_out=$(lsof -nP -i UDP:"${start_port}-${end_port}" 2>/dev/null || true)
  if [[ -z "$lsof_out" ]]; then
    echo "[pong-stack] no ICE port listeners detected" >&2
    return
  fi
  # Skip Docker/VPNKit plumbing so we don't kill the daemon; only target stray host processes.
  local pids
  pids=$(printf '%s\n' "$lsof_out" | awk 'NR>1 && $1 !~ /(docke|vpnkit|containerd)/ {print $2}' | sort -u)
  if [[ -z "$pids" ]]; then
    echo "[pong-stack] ICE ports are in use by docker-managed processes; skipping kill" >&2
    return
  fi
  echo "[pong-stack] killing PIDs using ICE ports: $pids" >&2
  kill -9 $pids 2>/dev/null || true
}

case "$COMMAND" in
  start)
    kill_conflicting_ice_ports
    ensure_service_running
    if ! wait_for_manager; then
      echo "[pong-stack] manager $MANAGER_URL did not become healthy; aborting start" >&2
      exit 1
    fi
    if ! wait_for_manager_ready; then
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
        if [[ "$CREATE_BEACH" == true ]]; then
          if [[ -n "$beach_id" ]]; then
            echo "[pong-stack] --create-beach set; creating a fresh beach instead of using provided id '$beach_id'" >&2
          fi
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
        if [[ -n "$beach_id" ]]; then
          # Forge credentials once for the session
          echo "[pong-stack] forging credentials..."
          get_forge_script > forge_token.py
          # Install dependencies if needed (assuming they are present or using system python)
          # We use the embedded python script to generate the credentials file
          if python3 forge_token.py > ~/.beach/credentials; then
              echo "[pong-stack] credentials forged successfully"
              # Update HOST_MANAGER_TOKEN
              HOST_MANAGER_TOKEN=$(grep "token =" ~/.beach/credentials | grep -v "dummy-refresh-token" | head -n 1 | cut -d '"' -f 2)
              export HOST_MANAGER_TOKEN
              HOST_TOKEN_EXPORT="export PB_MANAGER_TOKEN='$HOST_MANAGER_TOKEN'; export PB_MCP_TOKEN='$HOST_MANAGER_TOKEN'; export PB_CONTROLLER_TOKEN='$HOST_MANAGER_TOKEN'; "
              # Mirror credentials into the container so run-agent can read them directly.
              run_in_container "mkdir -p /root/.beach && chmod 700 /root/.beach"
              docker compose cp ~/.beach/credentials "$SERVICE:/root/.beach/credentials"
              run_in_container "chmod 600 /root/.beach/credentials"
          else
              echo "[pong-stack] failed to forge credentials"
              exit 1
          fi
          rm forge_token.py

          ensure_cli_login

          start_player lhs "LHS"
          if ! wait_for_role_bootstrap lhs "LHS player"; then
            stop_stack
            exit 1
          fi
          start_player rhs "RHS"
          if ! wait_for_role_bootstrap rhs "RHS player"; then
            stop_stack
            exit 1
          fi
          start_agent "$beach_id"
          if ! wait_for_role_bootstrap agent "Pong agent"; then
            stop_stack
            exit 1
          fi
        fi
        echo "waiting $CODES_WAIT seconds for sessions to register..."
        sleep "$CODES_WAIT"
        print_codes
        needs_setup=false
        if layout_missing "$beach_id"; then
          needs_setup=true
          echo "[pong-stack] detected missing or incomplete layout for $beach_id; auto-installing showcase graph..." >&2
        fi
        if ! sessions_attached_to_manager "$beach_id"; then
          needs_setup=true
          echo "[pong-stack] detected missing session attachments for $beach_id; auto-installing showcase graph..." >&2
        fi
        if [[ "$SETUP_BEACH" == true ]]; then
          needs_setup=true
        fi
        if [[ "$needs_setup" == true ]]; then
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

#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
usage: run-agent.sh <private-beach-id>

Environment variables:
  BEACH_PROFILE        Name of beach CLI profile (default: local)
  LOG_DIR              Directory for host logs (default: ~/beach-debug)
  BEACH_AUTH_GATEWAY   Auth gateway URL (default: http://localhost:4133)
  BEACH_AUTH_SCOPE     Auth scope (default: pb.full)
  BEACH_AUTH_AUDIENCE  Auth audience (default: private-beach)
  RUN_AGENT_MANAGER_URL Manager URL override (default: http://localhost:8080)
USAGE
}

if [[ $# -lt 1 ]]; then
  usage
  exit 1
fi

PRIVATE_BEACH_ID=$1
BEACH_PROFILE=${BEACH_PROFILE:-local}
LOG_DIR=${LOG_DIR:-"$HOME/beach-debug"}
echo "[run-agent] LOG_DIR=$LOG_DIR" >&2
mkdir -p "$LOG_DIR"

MANAGER_URL_DEFAULT=${RUN_AGENT_MANAGER_URL:-"http://localhost:8080"}
if [[ -z "${PRIVATE_BEACH_MANAGER_URL:-}" ]]; then
  export PRIVATE_BEACH_MANAGER_URL="$MANAGER_URL_DEFAULT"
elif [[ "${PRIVATE_BEACH_MANAGER_URL}" == "http://beach-manager:8080" ]]; then
  echo "[run-agent] overriding PRIVATE_BEACH_MANAGER_URL=http://beach-manager:8080 with $MANAGER_URL_DEFAULT for host-side connectivity" >&2
  export PRIVATE_BEACH_MANAGER_URL="$MANAGER_URL_DEFAULT"
fi

SESSION_SERVER=${RUN_AGENT_SESSION_SERVER:-${PONG_SESSION_SERVER:-http://localhost:4132/}}

export BEACH_AUTH_GATEWAY=${BEACH_AUTH_GATEWAY:-"http://localhost:4133"}
export BEACH_AUTH_SCOPE=${BEACH_AUTH_SCOPE:-"pb.full"}
export BEACH_AUTH_AUDIENCE=${BEACH_AUTH_AUDIENCE:-"private-beach"}

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "$SCRIPT_DIR/../../../../.." && pwd)
CREDENTIALS_FILE="${BEACH_CREDENTIALS_FILE:-$HOME/.beach/credentials}"

ensure_token() {
  python3 - "$BEACH_PROFILE" "$CREDENTIALS_FILE" <<'PY' || return 1
import os, sys
try:
    import tomllib as toml
except ModuleNotFoundError:
    import tomli as toml  # type: ignore

profile, path = sys.argv[1], sys.argv[2]
try:
    with open(os.path.expanduser(path), 'rb') as fh:
        data = toml.load(fh)
    token = data['profiles'][profile]['access_token']['token']
except Exception as exc:
    sys.stderr.write(f"token lookup failed: {exc}\n")
    sys.exit(1)
print(token, end='')
PY
}

login_cli() {
  echo "[run-agent] launching beach login for profile '$BEACH_PROFILE'..." >&2
  (cd "$REPO_ROOT" && cargo run --bin beach -- login --name "$BEACH_PROFILE" --force) >&2
}

obtain_cli_token() {
  if token=$(ensure_token); then
    printf '%s' "$token"
    return 0
  fi
  echo "[run-agent] no access token cached for profile '$BEACH_PROFILE'; launching beach login..." >&2
  login_cli || return 1
  if token=$(ensure_token); then
    printf '%s' "$token"
    return 0
  fi
  echo "[run-agent] unable to load access token after login" >&2
  return 1
}

refresh_cli_token() {
  login_cli || return 1
  if token=$(ensure_token); then
    printf '%s' "$token"
    return 0
  fi
  echo "[run-agent] unable to load access token after login" >&2
  return 1
}

USER_SUPPLIED_MANAGER_TOKEN=false
if [[ -z "${PB_MANAGER_TOKEN:-}" ]]; then
  if ! PB_MANAGER_TOKEN=$(obtain_cli_token); then
    exit 1
  fi
  export PB_MANAGER_TOKEN
else
  USER_SUPPLIED_MANAGER_TOKEN=true
  echo "[run-agent] using PB_MANAGER_TOKEN from environment" >&2
fi

USER_SUPPLIED_MCP=false
if [[ -z "${PB_MCP_TOKEN:-}" ]]; then
  export PB_MCP_TOKEN="$PB_MANAGER_TOKEN"
else
  USER_SUPPLIED_MCP=true
fi

USER_SUPPLIED_CONTROLLER=false
if [[ -z "${PB_CONTROLLER_TOKEN:-}" ]]; then
  export PB_CONTROLLER_TOKEN="$PB_MANAGER_TOKEN"
else
  USER_SUPPLIED_CONTROLLER=true
fi

sync_optional_tokens() {
  if [[ "$USER_SUPPLIED_MCP" == "false" ]]; then
    export PB_MCP_TOKEN="$PB_MANAGER_TOKEN"
  fi
  if [[ "$USER_SUPPLIED_CONTROLLER" == "false" ]]; then
    export PB_CONTROLLER_TOKEN="$PB_MANAGER_TOKEN"
  fi
}

check_manager_access() {
  local token="$1"
  local tmp
  tmp=$(mktemp)
  local status
  status=$(curl -s -o "$tmp" -w "%{http_code}" \
    -H "authorization: Bearer $token" \
    "$PRIVATE_BEACH_MANAGER_URL/private-beaches/$PRIVATE_BEACH_ID") || status=0
  CHECK_STATUS="$status"
  CHECK_BODY="$(cat "$tmp")"
  rm -f "$tmp"
}

sync_optional_tokens

# Sanity-check that the manager token can access the target private beach.
check_manager_access "$PB_MANAGER_TOKEN"
STATUS="$CHECK_STATUS"
if [[ "$STATUS" != "200" ]]; then
  if [[ "$USER_SUPPLIED_MANAGER_TOKEN" == "true" ]]; then
    echo "[run-agent] provided PB_MANAGER_TOKEN cannot access private beach '$PRIVATE_BEACH_ID' (HTTP $STATUS)." >&2
    echo "[run-agent] attempting fresh beach login for profile '$BEACH_PROFILE'..." >&2
    if ! PB_MANAGER_TOKEN=$(refresh_cli_token); then
      echo "[run-agent] fallback login failed; please verify credentials or scopes." >&2
      exit 1
    fi
    USER_SUPPLIED_MANAGER_TOKEN=false
  else
    echo "[run-agent] cached access token failed with HTTP $STATUS; refreshing beach login..." >&2
    if ! PB_MANAGER_TOKEN=$(refresh_cli_token); then
      exit 1
    fi
  fi
  export PB_MANAGER_TOKEN
  sync_optional_tokens
  check_manager_access "$PB_MANAGER_TOKEN"
  STATUS="$CHECK_STATUS"
  if [[ "$STATUS" != "200" ]]; then
    for attempt in 1 2 3; do
      sleep 1
      check_manager_access "$PB_MANAGER_TOKEN"
      STATUS="$CHECK_STATUS"
      if [[ "$STATUS" == "200" ]]; then
        break
      fi
    done
  fi
  if [[ "$STATUS" != "200" ]]; then
    echo "[run-agent] token still cannot access private beach '$PRIVATE_BEACH_ID' after login (HTTP $STATUS)." >&2
    if [[ -n "$CHECK_BODY" ]]; then
      echo "  • Response: $CHECK_BODY" >&2
    fi
    echo "  • Token prefix: ${PB_MANAGER_TOKEN:0:32}..." >&2
    echo "  • Profile: $BEACH_PROFILE" >&2
    echo "  • Manager: $PRIVATE_BEACH_MANAGER_URL" >&2
    echo "  • Ensure this account has pb:beaches.* / pb:sessions.* scopes for the beach." >&2
    exit 1
  fi
fi

LOG_FILE="$LOG_DIR/beach-host-agent.log"
BOOTSTRAP_FILE="$LOG_DIR/bootstrap-agent.json"

cd "$REPO_ROOT"

cargo run --bin beach -- \
  --log-level trace \
  --log-file "$LOG_FILE" \
  --session-server "$SESSION_SERVER" \
  host \
  --bootstrap-output json \
  --wait \
  -- /usr/bin/env python3 "$REPO_ROOT/apps/private-beach/demo/pong/agent/main.py" \
       --private-beach-id "$PRIVATE_BEACH_ID" \
       --mcp-base-url "http://localhost:8080" \
       --mcp-token "$PB_MCP_TOKEN" \
       --default-controller-token "$PB_CONTROLLER_TOKEN" \
       --lease-reason "pong_showcase" \
  | tee "$BOOTSTRAP_FILE"

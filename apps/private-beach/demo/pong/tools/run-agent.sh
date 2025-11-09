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
USAGE
}

if [[ $# -lt 1 ]]; then
  usage
  exit 1
fi

PRIVATE_BEACH_ID=$1
BEACH_PROFILE=${BEACH_PROFILE:-local}
LOG_DIR=${LOG_DIR:-"$HOME/beach-debug"}
mkdir -p "$LOG_DIR"

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

if ! token=$(ensure_token); then
  echo "[run-agent] no access token cached for profile '$BEACH_PROFILE'; launching beach login..." >&2
  (cd "$REPO_ROOT" && cargo run --bin beach -- login --name "$BEACH_PROFILE" --force)
  if ! token=$(ensure_token); then
    echo "[run-agent] unable to load access token after login" >&2
    exit 1
  fi
fi

export PB_MANAGER_TOKEN="$token"
export PB_MCP_TOKEN="$PB_MANAGER_TOKEN"
export PB_CONTROLLER_TOKEN="$PB_MANAGER_TOKEN"

LOG_FILE="$LOG_DIR/beach-host-agent.log"
BOOTSTRAP_FILE="$LOG_DIR/bootstrap-agent.json"

cd "$REPO_ROOT"

cargo run --bin beach -- \
  --log-level trace \
  --log-file "$LOG_FILE" \
  --session-server http://localhost:4132/ \
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

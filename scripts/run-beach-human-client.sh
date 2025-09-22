#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 2 ]]; then
  cat >&2 <<'USAGE'
Usage: run-beach-human-client.sh SESSION_ID PASSCODE [EXTRA_ARGS...]

Environment variables:
  BEACH_HUMAN_SESSION_SERVER       Base URL for the session broker (default: http://127.0.0.1:8080)
  BEACH_HUMAN_CLIENT_TIMEOUT_SECONDS  Seconds before sending SIGTERM (default: 30)
  BEACH_HUMAN_CLIENT_KILL_GRACE_SECONDS Seconds to wait before SIGKILL after timeout (default: 5)
USAGE
  exit 64
fi

SESSION_ID="$1"; shift
PASSCODE="$1"; shift

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")"/.. && pwd)"
TIMEOUT_SECONDS="${BEACH_HUMAN_CLIENT_TIMEOUT_SECONDS:-30}"
KILL_GRACE_SECONDS="${BEACH_HUMAN_CLIENT_KILL_GRACE_SECONDS:-5}"
SESSION_SERVER="${BEACH_HUMAN_SESSION_SERVER:-http://127.0.0.1:8080}"
BINARY_PATH="$REPO_ROOT/target/debug/beach-human"

if ! command -v timeout >/dev/null 2>&1; then
  echo "\033[31merror:\033[0m GNU timeout is required but was not found in PATH" >&2
  exit 127
fi

pushd "$REPO_ROOT" >/dev/null
cargo build -p beach-human
popd >/dev/null

exec timeout --signal=TERM --kill-after="${KILL_GRACE_SECONDS}s" "${TIMEOUT_SECONDS}s" \
  "$BINARY_PATH" --session-server "$SESSION_SERVER" join "$SESSION_ID" --passcode "$PASSCODE" "$@"

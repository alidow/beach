#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")"/.. && pwd)"
TIMEOUT_SECONDS="${BEACH_TIMEOUT_SECONDS:-30}"
KILL_GRACE_SECONDS="${BEACH_KILL_GRACE_SECONDS:-5}"
BINARY_PATH="$REPO_ROOT/target/debug/beach"

if ! command -v timeout >/dev/null 2>&1; then
  echo "\033[31merror:\033[0m GNU timeout is required but was not found in PATH" >&2
  exit 127
fi

pushd "$REPO_ROOT" >/dev/null

cargo build -p beach

popd >/dev/null

exec timeout --signal=TERM --kill-after="${KILL_GRACE_SECONDS}s" "${TIMEOUT_SECONDS}s" "$BINARY_PATH" "$@"

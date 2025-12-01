#!/usr/bin/env bash
set -euo pipefail

ROOT=/workspace/testing/webrtc-tester
RESULTS_DIR=${RESULTS_DIR:-"${ROOT}/results"}
mkdir -p "${RESULTS_DIR}"

HANDSHAKE_PATH=${HOST_HANDSHAKE_PATH:-"${RESULTS_DIR}/host-handshake.json"}
HOST_STDOUT_PATH=${HOST_STDOUT_LOG:-"${RESULTS_DIR}/host-stdout.log"}
HOST_STRUCTURED_LOG=${HOST_STRUCTURED_LOG:-"${RESULTS_DIR}/host.log"}
HOST_SUMMARY_PATH=${HOST_SUMMARY_PATH:-"${RESULTS_DIR}/host-summary.json"}
HOST_PAYLOAD_PREFIX=${HOST_PAYLOAD_PREFIX:-""}
ECHO_LOG_PATH=${ECHO_LOG_PATH:-"${RESULTS_DIR}/echo-server.log"}

rm -f "$HANDSHAKE_PATH" "$HOST_STDOUT_PATH" "$HOST_SUMMARY_PATH"
: >"$HOST_STDOUT_PATH"
: >"$HOST_STRUCTURED_LOG"
: >"$ECHO_LOG_PATH"

export BEACH_PUBLIC_MODE=${BEACH_PUBLIC_MODE:-1}
export BEACH_SESSION_SERVER=${BEACH_SESSION_SERVER:-http://signaling:5232}
export BEACH_LOG_FILE=${BEACH_LOG_FILE:-$HOST_STRUCTURED_LOG}
export BEACH_LOG_LEVEL=${BEACH_LOG_LEVEL:-info}
export RUST_LOG=${RUST_LOG:-webrtc::peer_connection=info,webrtc::ice_transport=info,beach::transport::webrtc=debug}
export BEACH_ICE_PORT_START=${BEACH_ICE_PORT_START:-62550}
export BEACH_ICE_PORT_END=${BEACH_ICE_PORT_END:-62650}
export ECHO_PREFIX="$HOST_PAYLOAD_PREFIX"
export ECHO_LOG_PATH
export PATH="/usr/local/cargo/bin:${PATH}"

if ! command -v python3 >/dev/null 2>&1; then
  apt-get update -y >/dev/null 2>&1
  apt-get install -y python3 >/dev/null 2>&1
fi

HOST_COMMAND=${HOST_COMMAND:-"python3 ${ROOT}/host/echo_server.py"}
BASE_URL="$BEACH_SESSION_SERVER"

write_summary() {
  python3 - <<'PY'
import json
import os
from pathlib import Path
import sys

handshake_path = Path(os.environ["HANDSHAKE_PATH"]).resolve()
summary_path = Path(os.environ["HOST_SUMMARY_PATH"]).resolve()
stdout_log = Path(os.environ["HOST_STDOUT_PATH"]).resolve()
structured = Path(os.environ["HOST_STRUCTURED_LOG"]).resolve()
if not handshake_path.exists():
    sys.exit(0)
try:
    handshake = json.loads(handshake_path.read_text())
except Exception:
    handshake = None
summary = {
    "status": "running",
    "handshake": handshake,
    "handshake_path": str(handshake_path),
    "stdout_log": str(stdout_log),
    "structured_log": str(structured),
}
summary_path.write_text(json.dumps(summary, indent=2))
PY
}

export HANDSHAKE_PATH HOST_SUMMARY_PATH HOST_STDOUT_PATH HOST_STRUCTURED_LOG

read -r -a host_words <<<"$HOST_COMMAND"
BIN=${BEACH_BIN:-/workspace/target/debug/beach}
if [ ! -x "$BIN" ]; then
  echo "Building beach binary (dev profile)..."
  cargo build -p beach --bin beach
fi
cmd=("$BIN" --log-level "$BEACH_LOG_LEVEL" --session-server "$BASE_URL" host --bootstrap-output json --)
cmd+=("${host_words[@]}")

echo "Starting beach host using session server ${BASE_URL}"

"${cmd[@]}" 2>>"$HOST_STDOUT_PATH" | {
  first=1
  while IFS= read -r line; do
    if [[ $first -eq 1 ]]; then
      printf '%s\n' "$line" | tee "$HANDSHAKE_PATH"
      first=0
      write_summary || true
    fi
    printf '%s\n' "$line"
  done
} | tee -a "$HOST_STDOUT_PATH"

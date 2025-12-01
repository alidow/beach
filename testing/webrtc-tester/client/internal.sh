#!/usr/bin/env bash
set -euo pipefail

ROOT=/workspace/testing/webrtc-tester
RESULTS=${RESULTS_DIR:-"${ROOT}/results"}
mkdir -p "$RESULTS"
export PATH="/usr/local/cargo/bin:${PATH}"

SESSION_SERVER=${BEACH_SESSION_SERVER:-http://signaling:5232}
PAYLOAD=${CLIENT_PAYLOAD:-"internal-$(date +%s%N)"}
LOG_FILE=${CLIENT_LOG_FILE:-"${RESULTS}/internal-client.log"}
CAPTURE=${CLIENT_CAPTURE:-"${RESULTS}/internal-client-capture.log"}
SUMMARY=${CLIENT_SUMMARY:-"${RESULTS}/internal-client-summary.json"}
HANDSHAKE=${HOST_HANDSHAKE_PATH:-"${RESULTS}/host-handshake.json"}
TIMEOUT=${CLIENT_TIMEOUT:-150}
HOLD_SECS=${CLIENT_HOLD_SECS:-0}
LABEL=${CLIENT_LABEL:-"internal-smoke"}

if ! command -v python3 >/dev/null 2>&1; then
  apt-get update -y >/dev/null 2>&1
  apt-get install -y python3 >/dev/null 2>&1
fi

: >"$LOG_FILE"
: >"$CAPTURE"

export BEACH_LOG_FILE=${BEACH_LOG_FILE:-$LOG_FILE}
export BEACH_LOG_LEVEL=${BEACH_LOG_LEVEL:-trace}
export RUST_LOG=${RUST_LOG:-beach::session=trace,beach::transport::webrtc=trace}

python3 "${ROOT}/client/run.py" \
  --mode internal \
  --session-server "$SESSION_SERVER" \
  --payload "$PAYLOAD" \
  --log-file "$LOG_FILE" \
  --capture "$CAPTURE" \
  --summary "$SUMMARY" \
  --handshake "$HANDSHAKE" \
  --label "$LABEL" \
  --timeout "$TIMEOUT" \
  --hold-secs "$HOLD_SECS"

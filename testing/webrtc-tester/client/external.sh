#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
RESULTS=${RESULTS_DIR:-"${ROOT}/results"}
mkdir -p "$RESULTS"
export BEACH_PUBLIC_MODE=${BEACH_PUBLIC_MODE:-1}
export BEACH_MANAGER_AUTH_OPTIONAL=${BEACH_MANAGER_AUTH_OPTIONAL:-1}
export BEACH_TOKEN=${BEACH_TOKEN:-}
export BEACH_SESSION_SERVER_BASE=${BEACH_SESSION_SERVER_BASE:-http://api.beach.dev:5232}
export BEACH_PUBLIC_SESSION_SERVER=${BEACH_PUBLIC_SESSION_SERVER:-http://api.beach.dev:5232}
export BEACH_INSECURE_TRANSPORT=${BEACH_INSECURE_TRANSPORT:-I_KNOW_THIS_IS_UNSAFE}

SESSION_SERVER=${BEACH_SESSION_SERVER:-http://api.beach.dev:5232}
PAYLOAD=${CLIENT_PAYLOAD:-"external-$(date +%s%N)"}
LOG_FILE=${CLIENT_LOG_FILE:-"${RESULTS}/external-client.log"}
CAPTURE=${CLIENT_CAPTURE:-"${RESULTS}/external-client-capture.log"}
SUMMARY=${CLIENT_SUMMARY:-"${RESULTS}/external-client-summary.json"}
HANDSHAKE=${HOST_HANDSHAKE_PATH:-"${RESULTS}/host-handshake.json"}
TIMEOUT=${CLIENT_TIMEOUT:-150}
HOLD_SECS=${CLIENT_HOLD_SECS:-0}
LABEL=${CLIENT_LABEL:-"external-smoke"}

python3 "${ROOT}/client/run.py" \
  --mode external \
  --session-server "$SESSION_SERVER" \
  --payload "$PAYLOAD" \
  --log-file "$LOG_FILE" \
  --capture "$CAPTURE" \
  --summary "$SUMMARY" \
  --handshake "$HANDSHAKE" \
  --label "$LABEL" \
  --timeout "$TIMEOUT" \
  --hold-secs "$HOLD_SECS"

#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"

./scripts/start.sh

echo "Running external client smoke against localhost:5232..."
CLIENT_SUMMARY=${CLIENT_SUMMARY:-"${ROOT}/results/external-client-summary.json"}
CLIENT_LOG_FILE=${CLIENT_LOG_FILE:-"${ROOT}/results/external-client.log"}
CLIENT_CAPTURE=${CLIENT_CAPTURE:-"${ROOT}/results/external-client-capture.log"}
CLIENT_PAYLOAD=${CLIENT_PAYLOAD:-"external-$(date +%s%N)"}
CLIENT_TIMEOUT=${CLIENT_TIMEOUT:-200}

BEACH_SESSION_SERVER=${BEACH_SESSION_SERVER:-http://localhost:5232} \
CLIENT_SUMMARY="$CLIENT_SUMMARY" \
CLIENT_LOG_FILE="$CLIENT_LOG_FILE" \
CLIENT_CAPTURE="$CLIENT_CAPTURE" \
CLIENT_PAYLOAD="$CLIENT_PAYLOAD" \
CLIENT_TIMEOUT="$CLIENT_TIMEOUT" \
  ./client/external.sh

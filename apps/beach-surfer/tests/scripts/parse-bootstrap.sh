#!/usr/bin/env bash
# Parse bootstrap JSON from Beach host session output
# Usage: parse-bootstrap.sh <bootstrap-file>

set -euo pipefail

BOOTSTRAP_FILE="${1:-}"

if [[ -z "$BOOTSTRAP_FILE" ]] || [[ ! -f "$BOOTSTRAP_FILE" ]]; then
  echo "Error: Bootstrap file not found: $BOOTSTRAP_FILE" >&2
  exit 1
fi

# Extract the JSON line from the bootstrap output
# The bootstrap output may contain ANSI codes and other terminal output
JSON_LINE=$(grep -o '{.*"session_id".*}' "$BOOTSTRAP_FILE" | head -1)

if [[ -z "$JSON_LINE" ]]; then
  echo "Error: No valid JSON found in bootstrap file" >&2
  echo "File contents:" >&2
  cat "$BOOTSTRAP_FILE" >&2
  exit 1
fi

# Extract session_id and join_code using grep/sed (no jq dependency)
SESSION_ID=$(echo "$JSON_LINE" | grep -o '"session_id":"[^"]*"' | cut -d'"' -f4)
JOIN_CODE=$(echo "$JSON_LINE" | grep -o '"join_code":"[^"]*"' | cut -d'"' -f4)
SESSION_SERVER=$(echo "$JSON_LINE" | grep -o '"session_server":"[^"]*"' | cut -d'"' -f4)

if [[ -z "$SESSION_ID" ]] || [[ -z "$JOIN_CODE" ]]; then
  echo "Error: Failed to parse session_id or join_code from JSON" >&2
  echo "JSON: $JSON_LINE" >&2
  exit 1
fi

# Output in easily-parseable format
echo "SESSION_ID=$SESSION_ID"
echo "PASSCODE=$JOIN_CODE"
echo "SESSION_SERVER=${SESSION_SERVER:-http://localhost:8080}"

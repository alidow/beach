#!/usr/bin/env bash
set -euo pipefail

ID="${1:-test-beach}"
BASE="${BASE_URL:-http://localhost:3000}"

echo "GET canvas layout (expect empty v3 graph)"
curl -sS "${BASE}/api/canvas-layout/${ID}" | jq .

echo "PUT canvas layout (simple graph)"
NOW=$(date +%s%3N)
payload=$(cat << JSON
{
  "version": 3,
  "viewport": { "zoom": 1, "pan": { "x": 0, "y": 0 } },
  "tiles": {
    "app-1": { "id": "app-1", "kind": "application", "position": { "x": 100, "y": 100 }, "size": { "width": 400, "height": 300 }, "zIndex": 1 }
  },
  "agents": {
    "agent-1": { "id": "agent-1", "position": { "x": 600, "y": 100 }, "size": { "width": 240, "height": 120 }, "zIndex": 2 }
  },
  "groups": {},
  "controlAssignments": {},
  "metadata": { "createdAt": ${NOW}, "updatedAt": ${NOW} }
}
JSON
)

curl -sS -X PUT "${BASE}/api/canvas-layout/${ID}" -H 'content-type: application/json' -d "${payload}" | jq .

echo "GET canvas layout (after save)"
curl -sS "${BASE}/api/canvas-layout/${ID}" | jq .

echo 'Done.'

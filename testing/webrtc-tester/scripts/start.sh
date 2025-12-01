#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"

echo "Bringing up WebRTC tester stack (signaling + host)..."
docker compose up -d signaling

# Wait for signaling to start accepting connections
echo "Waiting for signaling to be reachable on http://localhost:5232..."
for i in {1..30}; do
  if curl -sSf http://localhost:5232/ >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

docker compose up -d host

#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"

./scripts/start.sh

echo "Running internal client smoke (docker compose run internal-client)..."
docker compose run --rm internal-client

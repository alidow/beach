#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_DIR="$SCRIPT_DIR/../apps/private-beach-rewrite"
ENV_FILE="$APP_DIR/.env.local"

TOKEN="${PRIVATE_BEACH_MANAGER_TOKEN:-}"
BASE_URL="${PRIVATE_BEACH_MANAGER_URL:-}"

if [ -z "$TOKEN" ]; then
  echo "❌ PRIVATE_BEACH_MANAGER_TOKEN is not set. Export it in CI before running this script."
  exit 1
fi

{
  echo "PRIVATE_BEACH_MANAGER_TOKEN=$TOKEN"
  if [ -n "$BASE_URL" ]; then
    echo "PRIVATE_BEACH_MANAGER_URL=$BASE_URL"
  fi
  if [ -n "${NEXT_PUBLIC_PRIVATE_BEACH_MANAGER_URL:-}" ]; then
    echo "NEXT_PUBLIC_PRIVATE_BEACH_MANAGER_URL=$NEXT_PUBLIC_PRIVATE_BEACH_MANAGER_URL"
  fi
} > "$ENV_FILE"

echo "✅ Wrote rewrite env configuration to $ENV_FILE"

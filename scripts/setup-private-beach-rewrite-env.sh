#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_DIR="$SCRIPT_DIR/../apps/private-beach-rewrite"
ENV_FILE="$APP_DIR/.env.local"
EXAMPLE_FILE="$APP_DIR/.env.local.example"

if [ ! -f "$EXAMPLE_FILE" ]; then
  echo "❌ Missing $EXAMPLE_FILE. Repository may be out of date."
  exit 1
fi

if [ ! -f "$ENV_FILE" ]; then
  echo "📄 Creating $ENV_FILE from template…"
  cp "$EXAMPLE_FILE" "$ENV_FILE"
  echo "⚠️  Update PRIVATE_BEACH_MANAGER_TOKEN in $ENV_FILE with the value provided by WS-B."
else
  echo "✅ $ENV_FILE already exists."
fi

if ! grep -q "PRIVATE_BEACH_MANAGER_TOKEN" "$ENV_FILE"; then
  echo "⚠️  PRIVATE_BEACH_MANAGER_TOKEN missing from $ENV_FILE. Add it for SSR fetches to succeed."
fi

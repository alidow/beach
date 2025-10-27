#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "🏖️  Private Beach E2E Test Suite"
echo "================================"
echo ""

# Start infrastructure
echo "Starting test infrastructure..."
"$SCRIPT_DIR/start-private-beach-tests.sh"

# Source environment variables
if [ -f /tmp/beach-test-env.sh ]; then
  source /tmp/beach-test-env.sh
else
  echo "❌ Failed to source test environment variables"
  "$SCRIPT_DIR/stop-private-beach-tests.sh"
  exit 1
fi

echo ""
echo "================================"
echo "Running E2E Tests..."
echo "================================"
echo ""

# Run tests
cd "$PROJECT_ROOT/apps/private-beach"
npm run test:e2e:tile-resize

TEST_EXIT_CODE=$?

# Cleanup
echo ""
echo "================================"
echo "Cleaning up..."
echo "================================"
cd "$PROJECT_ROOT"
"$SCRIPT_DIR/stop-private-beach-tests.sh"

echo ""
if [ $TEST_EXIT_CODE -eq 0 ]; then
  echo "✅ All tests passed!"
  echo ""
else
  echo "❌ Tests failed (exit code: $TEST_EXIT_CODE)"
  echo ""
  exit $TEST_EXIT_CODE
fi

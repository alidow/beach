#!/usr/bin/env bash
set -euo pipefail

# Simple smoke harness to bring up a dedicated stack and optionally run the RTC bus test.
# Requires docker compose, and will use alternate ports (Road 14132, manager 18081, redis 16379).

COMPOSE_FILE="$(dirname "$0")/docker-compose.smoke.yml"
STACK_NAME=beach-smoke

start_stack() {
  echo "Starting smoke stack..."
  docker compose -f "$COMPOSE_FILE" up -d redis-smoke beach-road-smoke beach-manager-rewrite-smoke beach-host-smoke
}

stop_stack() {
  echo "Stopping smoke stack..."
  docker compose -f "$COMPOSE_FILE" down
}

wait_for_health() {
  echo "Waiting for beach-manager-rewrite-smoke on 18081..."
  for _ in {1..30}; do
    if curl -sf http://localhost:18081/health >/dev/null 2>&1; then
      echo "Manager rewrite is up"
      return 0
    fi
    sleep 1
  done
  echo "Manager rewrite did not become healthy in time" >&2
  return 1
}

run_rtc_test() {
  if [[ -z "${BEACH_RTC_TEST_HOST_SESSION_ID:-}" ]]; then
    echo "BEACH_RTC_TEST_HOST_SESSION_ID not set; skipping RTC bus test." ; return 0
  fi
  export BEACH_MANAGER_BUS_MODE=rtc
  export BEACH_ROAD_URL=${BEACH_ROAD_URL:-http://api.beach.smoke:14132}
  export BEACH_SESSION_SERVER_BASE=${BEACH_SESSION_SERVER_BASE:-http://api.beach.smoke:14132}
  cargo test -p beach-manager-rewrite rtc_bus -- --ignored --nocapture
}

check_cache_sync() {
  # Poll manager caches endpoint for host session if provided.
  if [[ -z "${BEACH_RTC_TEST_HOST_SESSION_ID:-}" ]]; then
    echo "BEACH_RTC_TEST_HOST_SESSION_ID not set; skipping cache sync check."
    return 0
  fi
  echo "Polling manager cache for host ${BEACH_RTC_TEST_HOST_SESSION_ID}..."
  for _ in {1..6}; do
    if curl -sf "http://localhost:18081/cache/${BEACH_RTC_TEST_HOST_SESSION_ID}" >/dev/null 2>&1; then
      echo "Cache endpoint responded for host ${BEACH_RTC_TEST_HOST_SESSION_ID}"
      return 0
    fi
    sleep 10
  done
  echo "Cache endpoint did not respond for host ${BEACH_RTC_TEST_HOST_SESSION_ID}" >&2
  return 1
}

long_smoke() {
  start_stack
  wait_for_health
  echo "Waiting 10s for host to register..."
  sleep 10
  if [[ -z "${BEACH_RTC_TEST_HOST_SESSION_ID:-}" ]]; then
    echo "BEACH_RTC_TEST_HOST_SESSION_ID not set; long smoke will only check health for 60s."
    sleep 60
  else
    echo "Running RTC bus test every 10s for 60s..."
    end=$((SECONDS+60))
    while [[ $SECONDS -lt $end ]]; do
      run_rtc_test || true
      sleep 10
    done
    check_cache_sync
  fi
}

case "${1:-}" in
  start)
    start_stack
    wait_for_health
    ;;
  stop)
    stop_stack
    ;;
  test)
    start_stack
    wait_for_health
    run_rtc_test
    ;;
  long)
    long_smoke
    ;;
  *)
    echo "Usage: $0 {start|stop|test|long}" >&2
    exit 1
    ;;
esac

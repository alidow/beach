#!/usr/bin/env bash
set -euo pipefail

# Simple smoke harness to bring up a dedicated stack and optionally run the RTC bus test.
# Requires docker compose, and will use alternate ports (Road 14132, manager 18081, redis 16379).

COMPOSE_FILE="$(dirname "$0")/docker-compose.smoke.yml"
STACK_NAME=beach-smoke
SMOKE_PASSPHRASE="${BEACH_SMOKE_PASSPHRASE:-SMOKEP}"
export BEACH_SMOKE_PASSPHRASE="${SMOKE_PASSPHRASE}"
export BEACH_WEBRTC_ATTACH_PASSPHRASE="${BEACH_WEBRTC_ATTACH_PASSPHRASE:-$SMOKE_PASSPHRASE}"

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
  for _ in {1..300}; do
    if curl -sf http://localhost:18081/health >/dev/null 2>&1; then
      echo "Manager rewrite is up"
      return 0
    fi
    sleep 1
  done
  echo "Manager rewrite did not become healthy in time" >&2
  return 1
}

set_host_env_from_logs() {
  logs=$(docker compose -f "$COMPOSE_FILE" logs beach-host-smoke 2>/dev/null || true)
  # Use the most recent session/passcode in case the container restarted and emitted multiple
  # registrations; picking the first entry can leave us with a stale host id that Road no longer
  # knows about (causing RTC attach 404s).
  host_id=$(echo "$logs" | awk '/session id/{id=$NF} END{print id}')
  host_pass=$(echo "$logs" | awk '/passcode/{pass=$NF} END{print pass}')
  if [[ -n "$host_id" ]]; then
    export BEACH_RTC_TEST_HOST_SESSION_ID="$host_id"
    echo "Detected host session id: $BEACH_RTC_TEST_HOST_SESSION_ID"
  fi
  if [[ -n "$host_pass" ]]; then
    export BEACH_WEBRTC_ATTACH_PASSPHRASE="$host_pass"
  fi
}

wait_for_host_registration() {
  echo "Waiting for host to register..."
  for _ in {1..120}; do
    set_host_env_from_logs
    if [[ -n "${BEACH_RTC_TEST_HOST_SESSION_ID:-}" ]]; then
      return 0
    fi
    sleep 2
  done
  echo "Host session id not detected; continuing without RTC test" >&2
  return 1
}

run_rtc_test() {
  if [[ -z "${BEACH_RTC_TEST_HOST_SESSION_ID:-}" ]]; then
    echo "BEACH_RTC_TEST_HOST_SESSION_ID not set; skipping RTC bus test." ; return 0
  fi
  # Run inside the manager container on the shared bridge to avoid host↔docker ICE issues.
  docker compose -f "$COMPOSE_FILE" exec -T beach-manager-rewrite-smoke bash -lc "
    set -euo pipefail
    export BEACH_RTC_TEST_HOST_SESSION_ID='${BEACH_RTC_TEST_HOST_SESSION_ID}'
    export BEACH_WEBRTC_ATTACH_PASSPHRASE='${BEACH_WEBRTC_ATTACH_PASSPHRASE:-$SMOKE_PASSPHRASE}'
    export BEACH_TURN_DISABLE=\${BEACH_TURN_DISABLE:-1}
    export BEACH_PUBLIC_MODE=\${BEACH_PUBLIC_MODE:-1}
    export BEACH_DISABLE_NAT_HINT=\${BEACH_DISABLE_NAT_HINT:-1}
    export BEACH_MANAGER_BUS_MODE=rtc
    export BEACH_ROAD_URL=\${BEACH_ROAD_URL:-http://beach-road-smoke:14132}
    export BEACH_SESSION_SERVER_BASE=\${BEACH_SESSION_SERVER_BASE:-http://beach-road-smoke:14132}
    export RUST_LOG=\"\${RUST_LOG:-debug,beach_manager_rewrite=debug,transport_webrtc=debug,beach::transport::webrtc=debug}\"
    export PATH=\"/usr/local/cargo/bin:\$PATH\"
    export CARGO_BUILD_JOBS=\${CARGO_BUILD_JOBS:-1}
    export RUSTFLAGS=\${RUSTFLAGS:--C codegen-units=1}
    cargo test -p beach-manager-rewrite rtc_bus -- --ignored --nocapture --test-threads 1
  "
}

touch_cache_action() {
  if [[ -z "${BEACH_RTC_TEST_HOST_SESSION_ID:-}" ]]; then
    return 0
  fi
  action_id="smoke-$(date +%s%N)"
  curl -sf -X POST http://localhost:18081/smoke/publish-action \
    -H 'content-type: application/json' \
    -d "{\"host_session_id\":\"${BEACH_RTC_TEST_HOST_SESSION_ID}\",\"controller_session_id\":\"smoke-controller\",\"action_id\":\"${action_id}\"}" >/dev/null 2>&1 || true
}

check_cache_sync() {
  # Poll manager caches endpoint for host session if provided.
  if [[ -z "${BEACH_RTC_TEST_HOST_SESSION_ID:-}" ]]; then
    echo "BEACH_RTC_TEST_HOST_SESSION_ID not set; skipping cache sync check."
    return 0
  fi
  echo "Polling manager cache for host ${BEACH_RTC_TEST_HOST_SESSION_ID}..."
  for _ in {1..6}; do
    resp=$(curl -sf "http://localhost:18081/cache/${BEACH_RTC_TEST_HOST_SESSION_ID}" || true)
    if [[ -n "$resp" ]]; then
      last_action=$(python - <<'PY' "$resp"
import json,sys
data=json.loads(sys.argv[1])
print(data.get("last_action_id") or "")
PY
)
      if [[ -n "$last_action" ]]; then
        echo "Cache shows last_action_id=${last_action}"
        return 0
      fi
      echo "Cache present but last_action_id missing; retrying..."
    fi
    sleep 10
  done
  echo "Cache endpoint did not show action for host ${BEACH_RTC_TEST_HOST_SESSION_ID}" >&2
  return 1
}

long_smoke() {
  start_stack
  wait_for_health
  wait_for_host_registration
  if [[ -z "${BEACH_RTC_TEST_HOST_SESSION_ID:-}" ]]; then
    echo "BEACH_RTC_TEST_HOST_SESSION_ID not set; long smoke will only check health for 60s."
    sleep 60
  else
    echo "Running RTC bus test once (cached build) and touching cache..."
    touch_cache_action
    run_rtc_test || true
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

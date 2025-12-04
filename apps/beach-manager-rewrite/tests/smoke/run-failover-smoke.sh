#!/usr/bin/env bash
set -euo pipefail

# Failover smoke: start two managers, assign host to manager-1, stop it, and ensure manager-2 takes over.

COMPOSE_FILE="$(dirname "$0")/docker-compose.failover.yml"
SMOKE_PASSPHRASE="${BEACH_SMOKE_PASSPHRASE:-SMOKEP}"
export BEACH_SMOKE_PASSPHRASE="${SMOKE_PASSPHRASE}"

start_stack() {
  echo "Starting failover smoke stack..."
  docker compose -f "$COMPOSE_FILE" up -d redis-smoke beach-road-smoke beach-manager-rewrite-smoke-1 beach-manager-rewrite-smoke-2 beach-host-smoke
}

stop_stack() {
  echo "Stopping failover smoke stack..."
  docker compose -f "$COMPOSE_FILE" down
}

wait_for_health() {
  local port="$1"
  echo "Waiting for manager on ${port}..."
  for _ in {1..300}; do
    if curl -sf "http://localhost:${port}/health" >/dev/null 2>&1; then
      echo "Manager on ${port} is up"
      return 0
    fi
    sleep 1
  done
  echo "Manager on ${port} did not become healthy in time" >&2
  return 1
}

set_host_env_from_logs() {
  logs=$(docker compose -f "$COMPOSE_FILE" logs beach-host-smoke 2>/dev/null || true)
  host_id=$(echo "$logs" | awk '/session id/{print $NF; exit}')
  host_pass=$(echo "$logs" | awk '/passcode/{print $NF; exit}')
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
  for _ in {1..300}; do
    set_host_env_from_logs
    if [[ -n "${BEACH_RTC_TEST_HOST_SESSION_ID:-}" ]]; then
      return 0
    fi
    sleep 2
  done
  echo "Host session id not detected" >&2
  return 1
}

attach_via_manager() {
  local port="$1"
  local host_id="$2"
  curl -sf -X POST "http://localhost:${port}/attach" \
    -H 'content-type: application/json' \
    -d "{\"host_session_id\":\"${host_id}\"}"
}

failover_smoke() {
  start_stack
  wait_for_health 18081
  wait_for_health 18082
  wait_for_host_registration
  host_id="${BEACH_RTC_TEST_HOST_SESSION_ID:-}"
  if [[ -z "$host_id" ]]; then
    echo "No host id detected; aborting" >&2
    exit 1
  fi

  echo "Assigning host to manager-1..."
  resp1=$(attach_via_manager 18081 "$host_id")
  echo "Manager-1 attach response: $resp1"
  assigned_here=$(echo "$resp1" | jq -r '.assigned_here')
  if [[ "$assigned_here" != "true" ]]; then
    echo "Manager-1 did not take assignment" >&2
    exit 1
  fi

  echo "Stopping manager-1 to simulate failure..."
  docker compose -f "$COMPOSE_FILE" stop beach-manager-rewrite-smoke-1
  echo "Waiting for assignment TTL to expire..."
  sleep 6

  echo "Requesting assignment via manager-2..."
  resp2=$(attach_via_manager 18082 "$host_id")
  echo "Manager-2 attach response: $resp2"
  assigned_here=$(echo "$resp2" | jq -r '.assigned_here')
  if [[ "$assigned_here" != "true" ]]; then
    echo "Manager-2 did not take assignment after failover" >&2
    exit 1
  fi
  echo "Failover smoke passed."
}

case "${1:-}" in
  start)
    start_stack
    ;;
  stop)
    stop_stack
    ;;
  run|long)
    failover_smoke
    ;;
  *)
    echo "Usage: $0 {start|stop|run}" >&2
    exit 1
    ;;
esac

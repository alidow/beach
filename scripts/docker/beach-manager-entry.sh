#!/usr/bin/env bash
set -euo pipefail

# Ensure Cargo binaries are available even when invoked from non-login shells.
export PATH="/usr/local/cargo/bin:${PATH:-}"

log() {
  echo "[beach-manager-entry] $*" >&2
}

resolve_host_ip() {
  local host_name="${BEACH_ICE_PUBLIC_HOST:-host.docker.internal}"
  local addr=""

  if command -v getent >/dev/null 2>&1; then
    addr="$(getent ahostsv4 "$host_name" | awk '{print $1; exit}' | tr -d '\r')"
  fi

  if [[ -z "$addr" ]] && command -v gethostip >/dev/null 2>&1; then
    addr="$(gethostip -4 "$host_name" 2>/dev/null | awk '{print $NF}' | tr -d '\r')"
  fi

  echo "$addr"
}

resolve_gateway_ipv4() {
  ip -4 route show default 2>/dev/null | awk '{print $3}' | head -n 1 | tr -d '\r'
}

resolve_gateway_ipv6() {
  ip -6 route show default 2>/dev/null | awk '{print $3}' | head -n 1 | tr -d '\r'
}

if [[ -z "${BEACH_ICE_PUBLIC_IP:-}" ]]; then
  candidate="$(resolve_host_ip)"
  if [[ -z "$candidate" ]]; then
    candidate="$(resolve_gateway_ipv4)"
  fi
  if [[ -z "$candidate" ]]; then
    candidate="$(resolve_gateway_ipv6)"
  fi

  if [[ -n "$candidate" ]]; then
    export BEACH_ICE_PUBLIC_IP="$candidate"
    log "BEACH_ICE_PUBLIC_IP not provided; detected ${BEACH_ICE_PUBLIC_IP}"
  else
    log "WARNING: unable to determine BEACH_ICE_PUBLIC_IP automatically; WebRTC fast-path may fail"
  fi
fi

exec "$@"

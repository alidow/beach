#!/usr/bin/env bash
set -euo pipefail

# Ensure Cargo binaries are available even when invoked from non-login shells.
export PATH="/usr/local/cargo/bin:${PATH:-}"

log() {
  echo "[beach-manager-entry] $*" >&2
}

# Historically we tried to auto-detect the host's LAN IP (by resolving
# host.docker.internal or the Docker gateway) and force Pion to use that via
# BEACH_ICE_PUBLIC_IP. That works only for a single-network setup; on laptops it
# routinely points at 192.168.65.254 which is *not* reachable by browsers, so
# ICE never succeeds. Instead we now rely on proper srflx/relay candidates via
# the configured STUN/TURN servers. When developers *really* need to pin an IP,
# they can still export BEACH_ICE_PUBLIC_IP/BEACH_ICE_PUBLIC_HOST explicitly.
if [[ -n "${BEACH_ICE_PUBLIC_IP:-}" ]]; then
  log "Using explicitly configured BEACH_ICE_PUBLIC_IP=${BEACH_ICE_PUBLIC_IP}"
elif [[ -n "${BEACH_ICE_PUBLIC_HOST:-}" ]]; then
  candidate=""
  if command -v getent >/dev/null 2>&1; then
    candidate="$(getent ahostsv4 "${BEACH_ICE_PUBLIC_HOST}" | awk '{print $1; exit}' | tr -d '\r')"
  fi
  if [[ -z "$candidate" ]] && command -v gethostip >/dev/null 2>&1; then
    candidate="$(gethostip -4 "${BEACH_ICE_PUBLIC_HOST}" 2>/dev/null | awk '{print $NF}' | tr -d '\r')"
  fi

  if [[ -n "$candidate" ]]; then
    export BEACH_ICE_PUBLIC_IP="$candidate"
    log "Resolved BEACH_ICE_PUBLIC_HOST=${BEACH_ICE_PUBLIC_HOST} -> ${BEACH_ICE_PUBLIC_IP}"
  else
    log "WARNING: BEACH_ICE_PUBLIC_HOST=${BEACH_ICE_PUBLIC_HOST} could not be resolved; proceeding without NAT hint"
  fi
else
  log "BEACH_ICE_PUBLIC_IP/HOST not set; relying on STUN for srflx/relay candidates"
fi

exec "$@"

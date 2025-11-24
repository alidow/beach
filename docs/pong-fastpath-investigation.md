# Pong Fast-Path: Current Failure State (Nov 22, 2025)

## Symptom
- Pong tiles connect over fast-path, but paddles/ball never move.
- Command traces and ball traces remain empty; hosts never apply controller bytes.
- Smoke runs (`scripts/pong-fastpath-smoke.sh`) report “No score updates / Missing ball trace”.

## What Works
- WebRTC chunking implemented on host/surfer.
- Manager `queue_actions` returns success and logs `queue_actions completed`.
- Manager logs show “fast-path actions delivered” for both player sessions.
- Fast-path handshakes complete for lhs/rhs/agent.

## What Fails
- Hosts never log “applied fast path controller bytes” (new host logging).
- Command traces (`/tmp/.../commands/command-{lhs,rhs}.log`) only contain the init line.
- Ball traces empty.
- In earlier manual runs, the agent’s SSE streams timed out and `queue_action` calls timed out; in later runs, `queue_action` reports success but delivery still does not reach hosts.

## Instrumentation Added
- **Manager**:
  - Log every `queue_actions` result (session_id, action_count, elapsed_ms, trace_id).
  - Warn on fast-path send failure before HTTP fallback; log channel missing at warn.
- **Host**:
  - Info log on data channel creation (label, peer, transport_id).
  - Info log when fast-path controller bytes are applied.
- **Log rotation**:
  - `pong-stack.sh` now creates `/tmp/pong-stack/<timestamp>` per run and updates `/tmp/pong-stack/latest`.

## Artifacts
- Latest failing smokes:
  - `temp/pong-fastpath-smoke/20251122-190708/`
  - `temp/pong-fastpath-smoke/20251122-194516/`
- Manager logs show `queue_actions completed` and `fast-path actions delivered`; no host-side applies.

## Hypotheses
- Manager may be sending fast-path actions on a different peer/channel than the host listener (need to verify with new host DC logs).
- HTTP fallback may not drain/forward despite success responses (queue stuck or wrong session).

## Blocker Right Now
- Stack health: `docker compose` intermittently complains `BEACH_ICE_PUBLIC_HOST/BEACH_ICE_PUBLIC_IP` missing even after `direnv allow`; smoke health checks fail. One workaround was exporting `BEACH_ICE_PUBLIC_HOST=192.168.68.52 BEACH_ICE_PUBLIC_IP=192.168.68.52` inline for compose.

## Next Steps (for whoever picks up)
1. Bring stack healthy (ensure ICE env vars visible to docker compose).
2. Rerun `scripts/pong-fastpath-smoke.sh`; collect `/tmp/pong-fastpath-smoke-<ts>/` plus `/tmp/pong-stack/latest` from manager.
3. Inspect host logs for:
   - `data channel created` entries (label/peer/transport_id).
   - `applied fast path controller bytes` entries.
4. Compare manager `fast_path.delivery` logs to host DC info to see if sends target the right channel.
5. If still no host receives, add host logging of inbound fast-path frames (even decode failures) and manager log of chosen peer/channel when sending.
6. Optional: run Playwright `pong-fastpath-live` with a real `PRIVATE_BEACH_ID` to confirm a healthy path.

## Env Notes
- If compose fails: export `BEACH_ICE_PUBLIC_HOST` and `BEACH_ICE_PUBLIC_IP` explicitly before invoking scripts.

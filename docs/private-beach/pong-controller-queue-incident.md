# Pong Controller Queue Incident Log

## Current Symptom
- Pong agent keeps backing off (`queue_action HTTP 429`) while both player tiles remain frozen at `last update XXs ago`.
- `beach-manager` logs show repeated `controller action queue over limit; queue_depth=500 …` for each player session (e.g. `f05a3dc4-…`, `50b9d7ea-…`).
- Redis queue never drains, so the agent’s writes are rejected and no rally starts.

## Key Observations
1. **Hosts never successfully auto-attach**
   - Every host boot prints `skipping auto-attach; PRIVATE_BEACH_ID or PRIVATE_BEACH_SESSION_PASSCODE not set`.
   - Immediately afterwards `receive_actions failed; unexpected status 404 {"error":"not_found","message":"session not found"}` loops forever in `beach-host-lhs.log` / `beach-host-rhs.log`.
   - Manager therefore has no `session_runtime.viewer_passcode`, so controller forwarders never start and queues accumulate.
2. **Manager queues overflow**
   - WARN lines (latest: `2025-11-11T23:08:38Z`) confirm each session queue hit the hard cap of 500 actions.
   - Once at the limit, every agent POST is throttled, producing the exponential backoff loop in the agent UI.
3. **Stale-session sweeper still trips**
   - Despite lowering `VIEWER_HEALTH_REPORT_INTERVAL` to 15 s and `STALE_SESSION_MAX_IDLE` to 60 s locally, the dockerized manager continues logging `session workers stopped … reason="stale_session_timeout"`, implying the container has not been rebuilt with the new constants.
4. **Layout API errors**
   - The rewrite UI is hitting `PUT /private-beaches/<id>/layout 422`, so the canvas layout never persists. No direct link to the queue issue yet, but it indicates the UI/backend contract is breaking elsewhere.

## What We’ve Tried (and Failed)
| Attempt | Result |
| --- | --- |
| Increased manager heartbeat frequency + stale timeout | No effect in logs (container likely still running old binary); stale-session timeouts persisted. |
| Added startup config logging in manager/road/rewrite | Verified env dumping works, but doesn’t resolve queue saturation. |
| Agent backoff logic & error reporting | Prevents log spam but still can’t deliver commands once queues are full. |
| Manual UI attachment of sessions | Manager shows `status="attached"` entries, yet hosts still see 404 because auto-attach path in the harness never executed. |
| Told user to set `PRIVATE_BEACH_ID/PASSCODE` manually | Rejected—by design, public demo sessions should remain unaware of private beach IDs. Need the beach-buggy ↔ manager handshake to occur automatically. |

## Open Hypotheses
1. **Handshake regression in beach-buggy host**
   - Host logs prove the new auto-attach helper in `apps/beach/src/server/terminal/host.rs` is never invoked. Either the helper is gated behind env vars (current behavior) or the manager handshake that previously populated passcodes is broken.
2. **Manager attach endpoint failing silently**
   - Need trace logs around `state.attach_by_code` to confirm whether CLI-initiated hosts ever reach that code path, whether verification fails, or if subsequent controller/viewer workers crash.
3. **Redis/controller forwarder misconfiguration**
   - Even after manual attach, queues still overflow, which suggests forwarders never mark actions as consumed. Need logs from `controller.forwarder` target to confirm connection status per session.
4. **UI layout 422**
   - The rewrite app currently can’t persist layout (`api.ts:747 422`). Although orthogonal, it means canvas state is constantly resubmitted, potentially triggering extra manager traffic/noise.

## Next Steps for Another Engineer
1. **Instrument beach-buggy host auto-attach**
   - Add trace logs around `maybe_auto_attach_session` and the CLI bootstrap path to confirm whether demo hosts ever collect the manager-issued passcode. If not, fix the handshake so hosts learn their private-beach metadata automatically.
2. **Add trace logging to `AppState::attach_by_code` and controller forwarder startup**
   - Need to know when attaches fail (403/422/etc.) and whether forwarders abort immediately.
3. **Inspect Redis queues directly**
   - Use `XLEN pb:<beach>:sess:<session>:actions` to validate depth and confirm no consumers exist.
4. **Root-cause the 422 layout error**
   - Check manager logs for the matching request to see why layout updates are rejected; ensures UI → manager contract isn’t broken for other APIs too.
5. **Rebuild and redeploy the CLI host whenever controller transport changes**
   - The fast_path `pb-controller` consumer now sends PTY bytes + InputAck directly; stale binaries fall back to HTTP-only mode and immediately overflow manager queues. Run `cargo build --bin beach` (or rebuild the docker image) before launching demo hosts so the automatic fast_path ↔ HTTP fallback switch engages.

## Nov 2025 Follow-up: Controller Channel Labels
- Beach Manager’s controller forwarder can only stream actions over a WebRTC data channel labelled `mgr-actions`, while CLI hosts historically registered a legacy `pb-controller` channel.
- Hosts paused their HTTP pollers as soon as any fast-path channel appeared, but the manager never recognized that channel, so every controller write fell back to the HTTP queue and eventually hit `queue_depth=500`.
- Fix:
  1. Standardize on `mgr-actions` for new builds (host + manager) while still accepting the legacy label for backward compatibility.
  2. Manager now attempts the modern label first and automatically retries with `pb-controller` if the remote host has not been upgraded.
  3. Update docs/helpful-commands to remind folks to rebuild the CLI/containers so the new label propagates everywhere.

## Jan 2026 Update: Chunked Fast-Path Framing
- The original incident also reproduced whenever the terminal snapshot exceeded ~16 KB: SCTP rejected the single-frame payload, stalled the data channel, and controller queues backed up even after the label fix.
- We now route every fast-path payload through the chunked framing layer (`crates/beach-buggy/src/fast_path.rs`), so large state diffs (and controller acks) are split into ≤14 KB envelopes before hitting WebRTC.
- Smoke tests (`scripts/fastpath-smoke.sh`) and Pong docs now enforce `rg 'payload_type="chunk"' beach-host-*.log` as part of validation. If that pattern never appears, rebuild the CLI + manager images before launching demo hosts to avoid reviving this incident.

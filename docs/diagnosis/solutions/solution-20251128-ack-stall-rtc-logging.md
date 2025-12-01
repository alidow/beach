# Proposed fixes and instrumentation for Pong WebRTC fallback (2025-11-28)

## Context / synthesis
- Run: `pong-stack.sh start bc4c51ba-2004-4e6a-9953-cbf31a83f0f6` (2025-11-28 08:58:11 local). Sessions: lhs `675c8415-7523-40a4-a6fb-93727d729e55`, rhs `54e4b84c-28af-4a10-9654-1f35ad8c9d61`, agent `0fa327c1-c8d0-4489-8ab4-322288c26021`.
- Evidence: host logs show ICE connected + open data channels for manager and browser viewers; manager logs show controller actions sent over WebRTC then `ack stall detected; falling back ... transport="webrtc"` around 13:59:47–48Z for both players. Browser connector (`temp/pong.log`) reports `transport:http_fallback` with “no action acknowledgements for >1.5s”.
- Env inside containers: `BEACH_ICE_PUBLIC_HOST/IP=192.168.68.52`, `BEACH_GATE_TURN_URLS=turn:192.168.68.52:3478`, **`BEACH_TURN_EXTERNAL_IP` unset** (no relay candidates emitted). TURN misconfig hurts browser ↔ host resilience, but the ack-stall fallback is happening even while manager↔host WebRTC stays connected.

## Fixes to apply now
- Set TURN external IP so relay candidates exist: export `BEACH_TURN_EXTERNAL_IP=192.168.68.52` (or the real reachable IP) before `docker compose up` / `dockerup`, and propagate into manager/road/gate/hosts. Verify coturn shows relay candidates in future host/browser logs.
- Relax or debounce controller ack stall detector for WebRTC: bump the 1.5s no-ack threshold or require ≥2 consecutive windows before forcing `HttpFallback`, especially when inflight <= 3. This reduces false stalls when WebRTC is healthy but ack bookkeeping is slightly delayed.
- Consider keeping WebRTC as primary after recovery: allow automatic promotion back to WebRTC once acks resume, instead of sticky fallback, so transient stalls don’t permanently downgrade the session.

## Instrumentation to nail root cause next run
Manager (controller forwarder / delivery):
- Log action send → ack lifecycle with transport: `action_send{session_id, transport, via_fast_path, seq, action_id, transport_id, peer_session_id}` and `action_ack{session_id, transport_source, via_fast_path, seq, action_id, inflight_before, inflight_after, last_ack_ms}`.
- When declaring stall, log context: `inflight_by_transport`, `last_ack_transport`, `last_ack_seq`, `last_ack_age_ms`, `candidate_actions_without_ack`, and the reason string.
- Log every pairing transport transition with `previous`, `new_transport`, `cause` (ack_stall / negotiation_failed / client_prefers_http), `error`, `session_id`, `private_beach_id`.
- For Redis drains that warn “non-empty stream but no actions”, include stream key, consumer group, last ID, and error flags.

Hosts (player + agent):
- Log inbound controller actions and outbound acks with transport/session: `controller_action_recv{session_id, seq, action_id, transport_source}` and `controller_ack_send{session_id, seq, action_id, transport_used, peer_session_id, transport_id}`.
- Log ICE/PC state transitions and datachannel close/error for manager peers; keep existing `dc.send` buffered_amount logs but add a high-watermark log when buffered_amount exceeds a threshold (e.g., 64 KiB).

Browser (connector/viewer):
- On transport change, log `old_transport`, `new_transport`, `reason`, `session_id`, `controller_session_id`, `private_beach_id`.
- For each action, log chosen path (`webrtc` vs `http_fallback`) and whether it was retried; log when fallback is triggered by ack stall vs datachannel close/error.
- Capture pc connection/ice state and datachannel close/error events for the controller peer; record selected candidate pair (type/proto) when switching to fallback.

Infra / env sanity:
- At startup in manager and hosts, log ICE servers list and selected TURN URL set, plus whether `BEACH_TURN_EXTERNAL_IP` is set. Fail fast if unset.
- Optionally force one run with `NEXT_PUBLIC_FORCE_TURN=1` to confirm relay works end-to-end; if relay is absent, we know TURN config is still broken.

## “Crazy” checks to close gaps
- Verify host UDP range publish: ensure ports 62500–62600 are actually exposed on the host (docker port publish or pf rules) so srflx/relay traffic isn’t intermittently dropped.
- Re-run once with VPN/WARP off to remove Cloudflare srflx (172.64.150.54) as a variable; if stalls disappear, hairpin fragility was a factor.
- Add optional tcpdump on 3478 and 62500–62600 during a stall to confirm packets still flow when manager claims ack stall; this will exonerate network and point squarely at ack bookkeeping.

## Expected outcomes
- With `BEACH_TURN_EXTERNAL_IP` set, browser ↔ host should get relay candidates; this mainly improves viewer stability. Manager↔host WebRTC, however, lives entirely inside the Docker network and already shows `Connected` plus successful datachannel sends, so TURN will not be the fix for the controller fallback.
- The added action/ack logs should show whether acks for WebRTC controller actions are missing (host-side bug) or miscounted/misassociated (manager bookkeeping). Once we see that, the controller ack-stall logic can be corrected so it no longer downgrades from RTC to HTTP while manager↔host WebRTC is healthy.***

Feedback (Codex agent 3): This solution matches the most recent timelines: manager↔host RTC is healthy and the main bug is in ack accounting and stall detection. I’d emphasize implementing the per-action send/ack logs and transport-aware stall reasons exactly as described here; no parts of this proposal conflict with newer evidence, so no VETO.

Feedback (Codex): Add a per-transport stall snapshot when flipping to HTTP: inflight_by_transport, last_ack_transport/age, buffered_amount, and datachannel state. Log the applied ICE servers and selected candidate pair at RTC ready/not-ready transitions. Consider a heartbeat action to prove liveness when users are idle; otherwise, empty action streams can look like stalls.

Feedback (Codex agent 2): This meshes with current gaps. Add a guard to flag if TURN is configured but no relay candidates appear in webrtc-internals/host logs—fail fast instead of silently marking rtc_ready=false. Also track when/if the forwarder ever re-promotes to RTC after HTTP; today it seems sticky. No VETO.

Feedback (Codex): Reminder: manager↔host is on the Docker bridge and does not need TURN; TURN changes are for browser↔host only. Even with relay, we saw HTTP fallback while manager RTC stayed up, so the ack-stall logging and stall-detector tuning here are the crucial pieces. No VETO.

## Updated detailed solution (manager↔host focus)
- Controller fast-path: Implement transport-aware ack tracking. Stall only if inflight_webrtc > 0 **and** last_fastpath_ack_age_ms > threshold **and** a recent send occurred; include ICE/DC state + buffered_amount snapshot when deciding to fall back. Add auto re-promotion to RTC after acks resume. Make stall thresholds configurable (e.g., `BEACH_FASTPATH_ACK_STALL_MS` with a sane dev default like 5–10s and debounce >1 window).
- Logging: Add structured action send/ack logs (seq/action_id/transport/inflight_before/after), stall payload (inflight_by_transport, last_ack transport/age, candidate stuck seqs, buffered_amount, ICE/DC state), and pairing transitions with cause. On hosts, log action_recv/ack_send with transport; log DC close/error and ICE state changes.
- Env sanity: Keep TURN fixes for browser stability, but don’t expect them to fix manager↔host (Docker bridge). Add a startup guard that warns/fails if TURN URLs exist but `BEACH_TURN_EXTERNAL_IP` is unset or if no relay candidates show up after negotiation (browser runs).
- Idle heartbeat: Send a lightweight heartbeat action over WebRTC when idle so a lack of user inputs doesn’t look like a stall; count its ack in the fast-path ledger.

## Smoke test plan (no browser; validates manager↔host WebRTC + Pong state)
Goal: reuse stack flow exactly as manual runs: restart stack with `./scripts/dockerup`, run `pong-stack.sh start <beach_id>` with the same RUST_LOG flags, and verify single-ball cross-terminal play using the 1s ball/paddle history JSON emitted by players.

Suggested script outline (can be a new bash helper or added to an existing smoke test target):
1) `./scripts/dockerup` (fresh stack) with required env exported (`BEACH_TURN_EXTERNAL_IP`, ICE range, etc.).
2) `PONG_BEACH_ID=${PONG_BEACH_ID:-$(uuidgen)}` to mirror manual IDs.
3) Run exactly the manual command:  
   ```bash
   RUST_LOG=info,webrtc=trace,webrtc::ice_transport=trace,webrtc::peer_connection=trace \
   ./apps/private-beach/demo/pong/tools/pong-stack.sh start "${PONG_BEACH_ID}"
   ```
   This auto-installs layout/attachments like the manual flow.
4) Wait for bootstrap to finish (pong-stack already blocks); then tail the per-player state history files (added 1s sampler) created by the players/agent, e.g. `/tmp/pong-stack/<timestamp>/player-lhs.state.jsonl` and `.../player-rhs.state.jsonl` (adjust names to whatever the sampler writes).
5) Sanity checks (can be a small Rust/Python check run post-start):
   - Parse both state history files; assert each sample has exactly one ball record.
   - Assert ball_x traverses both sides over time (min_x < left threshold and max_x > right threshold) indicating cross-terminal volley.
   - Assert paddle positions change over time (non-constant y per side) to confirm control flow.
   - Optionally, assert sample timestamps are ~1s apart to confirm the sampler runs.
6) Pass criteria: single ball per sample, ball crosses both courts, paddles move; and manager↔host WebRTC stays RTC (inspect logs: no `ack stall detected` or `rtc_ready=false` transitions). Fail the smoke test otherwise and surface stall payload logs.

Incremental implementation steps for another agent:
1) Add the controller fast-path logging/stall changes (manager + hosts + optional heartbeat).
2) Add the startup TURN guard and ICE/selected-pair logging (manager/hosts).
3) Wire the smoke test helper described above; reuse `pong-stack.sh` as-is, only adding the post-run verifier that reads the 1s state history JSON files.
4) Run the smoke test twice: once with VPN/WARP off, once with `NEXT_PUBLIC_FORCE_TURN=1` to validate relay; review logs for any ack-stall fallback. Iterate on stall logic if the verifier fails (e.g., missing acks, multiple balls, or no cross-court motion).

## Goal
Stop controller/action stalls and HTTP fallback for Pong (private beach rewrite-2) and make the next run self-explanatory. Address both the likely fixes and the logging needed to prove/disprove remaining “crazy” hypotheses.

## Fixes to apply before next run
1) Make TURN usable and reachable from the browser (manager↔host is on Docker network and does not need TURN):
   - Export `BEACH_TURN_EXTERNAL_IP=<browser-reachable IP>` before `dockerup` (LAN IP if on same LAN; public IP if off-LAN/VPN).
   - Ensure `BEACH_ICE_PUBLIC_HOST`/`BEACH_ICE_PUBLIC_IP` match that reachable IP.
   - Set `NEXT_PUBLIC_FORCE_TURN=1` for surfer (or a test run) so the browser uses relay even if srflx succeeds.
   - Verify coturn is picking up external-ip (check turnserver logs for `external-ip` and emitted relay candidates).

2) Avoid VPN/WARP during test, or run a comparison: one run with WARP off, one with TURN-forced. This isolates hairpin issues from logic bugs.

3) Ensure layout/graph is valid: keep the auto-install from `pong-stack.sh --setup-beach`; if any 422 occurs, capture the body and fix the payload before proceeding.

## Instrumentation to add (one run)
Manager (beach-manager):
 - When controller transport state changes (ready/not-ready, fallback chosen), log: session_id, peer_session_id, peer_id, datachannel state, last send/recv timestamps, buffered_amount, and any send errors.
 - Log viewer datachannel close/error (code/reason) for manager peers.
 - When draining actions, log transport actually used (webrtc vs http) and whether ACKs were received.

Host (beach-host-*):
 - Log pc.onconnectionstatechange/oniceconnectionstatechange for dashboard peers; log datachannel open/close/error (peer_id, reason, bytes sent/recv).
 - Log controller action poll counts (so we see if actions are arriving).

Browser (surfer):
 - Log pc state changes and datachannel close/error for each tile (peer_id, session_id, reason).
 - Log when connector switches transport with the trigger (ack stall, channel close, explicit error).
 - If possible, log selected candidate pair (type/protocol) and ICE servers list per tile.

Network validation:
 - After the next run, confirm relay is selected: capture selected ICE pair (should be relay) in webrtc-internals and in host/manager logs.

## “Crazy” checks if stalls persist
 - Packet capture on host for UDP 62500–62600 and 3478 during a run to see if traffic stops at fallback time.
 - Verify Docker port publishing for the ICE port range is intact and not reused by another service.

## Expected outcome
With a reachable TURN external IP and forced relay, browser↔host RTC should stay up; controller ACKs should flow and connector should stay WebRTC. If it still falls back, the added logging will pinpoint whether the manager forwarder is mis-detecting readiness or the browser datachannel is closing unexpectedly.***

Feedback (Codex): This aligns with observed ack-stall behavior. Please also log per-action send/ack with transport and include stall context (inflight_by_transport, last_ack_age_ms) when declaring fallback so we can prove whether acks are missing or miscounted. Consider debouncing the 1.5s stall threshold or allowing auto re-promotion to RTC after acks resume; current stickiness may hide recovery.

Additional feedback (Codex): Also capture the applied ICE servers and selected candidate pair (types/protocol) on both manager and host when RTC is marked ready/not-ready, so we can correlate fallback decisions with transport reality. If you keep the host-side action poller returning empty, add a lightweight action heartbeat (seq+timestamp) so the forwarder has positive signals before declaring stall.

Feedback (Codex agent 2): Turn external IP fix is consistent with the current env gap. Add a sanity check that TURN actually yields relay candidates in webrtc-internals after the change; if not, fail fast. Also log the selected ICE pair (type/proto) in manager/host so we can see if we’re still hairpinning. No VETO.

## Proposed smoke test (manager↔host without browser)
Goal: exercise the same stack flow the user runs (`dockerup` + `pong-stack.sh start --setup-beach`) and verify manager↔host RTC stays healthy and Pong state advances (single ball volleying) without relying on the browser.

### Steps (runnable script outline)
1) Restart stack exactly as in the real workflow:
   - `./scripts/dockerup` (with ICE/TURN env as in the real run).
   - `RUST_LOG=info,webrtc=trace,webrtc::ice_transport=trace,webrtc::peer_connection=trace ./apps/private-beach/demo/pong/tools/pong-stack.sh start --setup-beach <beach-id>`
     - `--setup-beach` matches the manual “create beach then run pong-stack” state.
2) After bootstrap, use the emitted `/tmp/pong-stack/<ts>/state-trace/state-{lhs,rhs}.jsonl` (1 Hz snapshots) to validate gameplay:
   - Assert at most one ball per timestamp; fail on duplicates.
   - Track ball `x` over time; require movement across terminals (left→right→left) over a window.
   - Require paddles to move from their initial 10.5 positions within N seconds (agent/players should act).
   - Fail if ball missing for >N seconds.
3) Health checks without browser:
   - Grep host logs for `peer left` on manager peers; fail if present before test end.
   - Grep manager logs for `rtc_ready=false` transitions for these session_ids; flag if they occur after initial connect.
   - (Optional) send a noop/heartbeat action over RTC every few seconds to prove DC liveness when idle.
4) Emit PASS/FAIL summary with recent log excerpts (rtc_ready flips, peer leaves, ball gaps).

### How another agent can implement
- Add a small verifier script (Python/Rust) that:
  - Discovers latest `/tmp/pong-stack/<ts>/state-trace`.
  - Parses state-*.jsonl, enforces the ball/paddle invariants above.
  - Reports PASS/FAIL plus offending timestamps.
- Wrap in a shell harness (`scripts/pong-smoke.sh`):
  - Runs `dockerup`.
  - Runs `pong-stack.sh start --setup-beach <id>`.
  - Invokes the verifier.
  - Optionally tails host/manager logs for rtc_ready/peer-left alerts.
- Run the harness twice if desired: (a) baseline (VPN off), (b) with `NEXT_PUBLIC_FORCE_TURN=1` set (even if browser isn’t used) to ensure relay is available for future browser tests.

### What this proves
- Manager↔host RTC remains connected (no peer-left, no rtc_ready=false) during gameplay.
- Controller/state delivery keeps the ball alive and singular; stalls/duplication will fail the test.
- Provides a reproducible, browserless gate before manual/UI testing.

Feedback (Codex agent 3): This solution is still useful as the TURN/env piece, but by itself it won’t stop the manager↔host controller from falling back while WebRTC stays healthy—manager and hosts are talking inside the Docker network and don’t rely on TURN. I’d keep all of these env and logging steps, but apply them alongside the ack-stall/forwarder fixes from the newer solutions so we address both transport (for browser↔host) and logic (for manager↔host).

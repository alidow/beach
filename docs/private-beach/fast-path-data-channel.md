# Controller Fast-Path Dropout (Nov 18, 2025)

## Summary
We solved the earlier gating bug (manager now falls back to HTTP until the fast-path data channel is registered), but the headless fast-path integration harness still fails the gameplay checks. The manager logs confirm it delivers controller actions over `mgr-actions`, yet the host never shows the corresponding `controller.actions.fast_path.apply` entries, and the Pong players never see movement. The latest instrumentation (fast-path IDs, peer IDs, and Beach Road answer logging) shows that the manager is indeed sending on a live `mgr-actions` channel, but the host is likely listening on a different peer connection after a sequence of `/sessions/{id}/webrtc/answer` 404s. We need a fresh set of eyes on why the bytes disappear between the manager delivery log and the host PTY writer.

## Reproduction
1. Ensure Docker + direnv env vars are configured (`BEACH_ICE_PUBLIC_*`, `PRIVATE_BEACH_MANAGER_TOKEN`).
2. From repo root run:
   ```bash
   FASTPATH_KEEP_STACK=1 direnv exec . python3 scripts/fastpath-integration.py
   ```
   (The harness now pauses 25 s before issuing commands and requests 120 s controller lease TTLs.)
3. When the run fails, inspect `temp/fastpath-integration/`:
   - `manager.log` – shows `fast-path actions delivered` (with `fast_path_id=N`) for both controller sessions as soon as the harness submits `queue_actions`.
   - `beach-host-{lhs,rhs}.log` – data channel comes up (`fast path controller channel ready peer=<uuid>`) but there are **no** `received fast path input frame` / `controller.actions.fast_path.apply` entries afterwards, only repeated `recv timed out` warnings.
   - `beach-road.log` (from `docker logs beach-road`) – alternates between `served webrtc answer` and `webrtc answer unavailable (404)` for both `/sessions/{id}/webrtc/answer` and `/fastpath/.../answer`, often logging multiple 404s before finally returning a 200.
   - `command-trace-*.log`, `ball-trace-*.jsonl`, `*-frame-*.txt` – remain empty/unchanged.

## What We Know So Far
- Manager gating fix works: `queue_actions` no longer rejects commands unless the session is FastPath-only **and** HTTP isn’t ready, and `cargo test -p beach-manager fast_path` passes.
- `fastpath-integration.py` waits ~25 s and renews controller leases (TTL=120 s) before queuing rally commands. It also enables frame dumps + ball traces via `PONG_FRAME_DUMP_DIR` / `PONG_BALL_TRACE_PATH`.
- Manager confirms fast-path delivery (action_count=1 per controller) and immediately updates pairing transport status to `fast_path`. We now log a `fast_path_id` so every data-channel open/send can be traced to a specific FastPathSession instance, plus the peer IDs, ICE states, and channel labels.
- Hosts negotiate WebRTC fast-path (`beach-host-lhs.log` around lines 7 9xx), pause the HTTP poller, and mark the channel as active, but even with extra instrumentation (`received fast path input frame`) no frames ever hit `run_fast_path_controller_channel`.
- Command/ball traces remain empty; frame dumps show no ball glyphs; Pong is still idle.
- During host negotiation the WebRTC client repeatedly logs `fetch 404; skipping fastpath alt for answer … alt_base=http://host.docker.internal:4132/fastpath/...` for both the main answer URL and the `/fastpath` variant. The manager still reports the fast-path session attached, but it looks like the host never receives those SDP answers in time, so it brings up a different peer connection via the fallback viewer path.
- Beach Road now logs every `/webrtc/answer` hit, so we can see sequences like: multiple 404s for the same `handshake_id`, then a final 200 with `client_peer_id=dbf327ff-…`. Those peer IDs match the host logs, but the `fast_path_id` referenced by the manager’s delivery logs often corresponds to an earlier peer connection that never gets promoted by the host.
- No secure-transport counter mismatch warnings appear in the latest manager.log, so the earlier suspicion about AEAD counter drift doesn’t apply to this run (though older logs did show `secure transport counter mismatch` errors when the host retried HTTP after fast-path timeouts).

## Latest Instrumentation Findings
- `apps/beach-manager/src/routes/fastpath.rs` now logs the `fast_path_id`, `session_id`, and `client_peer_id` when creating the offer/answer, plus the ICE/data-channel lifecycle. Example from `manager.log`:
  ```
  fast_path_session.created fast_path_id=2 session_id=8cfbdf5f-... peer_id=743c...
  fast_path_session.data_channel_open fast_path_id=2 label="mgr-actions"
  fast_path_actions.delivered fast_path_id=2 session_id=8cfbdf5f-... action_count=1
  ```
- Hosts log `fast path controller channel ready transport_id=TransportId(1) peer=dbf327ff-...`, but never log `received fast path input frame`. We added temporary instrumentation in `run_fast_path_controller_channel` that would emit the payload length/seq if it ever saw a `WireClientFrame::Input`, and no such log appears.
- Beach Road logs (new handler instrumentation) show bursts of `webrtc answer unavailable (404)` before eventually serving an answer for the same session/handshake, suggesting that either the manager hasn’t registered the FastPathSession yet or we’re querying with stale IDs.
- Harness stores all logs under `temp/fastpath-integration/`; the latest bundle (Dec 2 run) shows the manager sending on `fast_path_id=4`, while the host promotes `transport_id=1` with `peer=dbf327ff-...`. We never log that `fast_path_id=4` is associated with that peer.

## Working Theory
The manager creates a FastPathSession proactively (and brings up its own WebRTC peer connection) when it receives `/fastpath/.../offer`, but the host’s UI establishes a separate controller transport later via the regular viewer signaling path. When `queue_actions` fires, the manager pulls the `fast_path_id` session from the registry and writes to its `mgr-actions` data channel—the one created during the proactive FastPathSession negotiation. Meanwhile, the host is listening on a completely different peer connection instantiated after the 404 retries. Because WebRTC data channels are tied to a specific RTCPeerConnection, the frames never reach the host listener. This aligns with the sequence observed in the logs: fast-path data channels open 20–30 s before the host even logs “controller channel ready,” and the peer IDs do not match.

## Open Questions for Reviewers
- Are the `WireClientFrame::Input` payloads being encoded or chunked incorrectly between `apps/beach-manager/src/fastpath.rs::send_actions_over_fast_path` and `apps/beach/src/server/terminal/host.rs::run_fast_path_controller_channel`?
- Could the manager be sending frames to a different data channel instance (e.g., duplicate `mgr-actions` channels) that the host isn’t listening to? The new `fast_path_id` logs suggest this happens whenever the proactive FastPathSession and the eventual host-promoted transport diverge.
- Why does the host keep logging 404s when fetching `/sessions/{id}/webrtc/answer` (and the `/fastpath` variant) even though the manager claims the FastPathSession is attached? Are we racing the answer endpoint or serving it on a different path? Beach Road now logs every request, so we can correlate by `handshake_id`.
- Once `/sessions/{id}/webrtc/answer` finally succeeds, how do we ensure the host and manager agree on which peer connection carries the `mgr-actions` channel? Should we re-bind the FastPathSession to the controller forwarder transport when the host promotes it?
- If the multiple-peer-connection theory is wrong, what else would explain the manager claiming success while the host never receives `WireClientFrame::Input`? Could the binary payload be rejected silently by the host transport layer?

Any guidance on why manager delivery logs don’t correspond to host reads—especially around the new binary fast-path encoding—would be super helpful.

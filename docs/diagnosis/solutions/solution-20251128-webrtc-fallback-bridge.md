# Proposed fixes and logging for WebRTC fallback / controller stall (bc4c51ba-2004-4e6a-9953-cbf31a83f0f6)

## Summary of observed issues
- Manager↔host WebRTC negotiates and remains up (answers served; has_webrtc_channels=true).
- Browser↔host WebRTC connects, then drops after ~1 minute; UI flips to `http_fallback`.
- Controller actions appear empty/stalled even while RTC is up; manager later marks `rtc_ready=false` and drains over HTTP.
- No relay candidates seen; TURN external IP unset → relay suppressed; srflx/host only → hairpin/keepalive fragile.

## Fixes to apply
1) **Turn/relay correctness**
   - Set `BEACH_TURN_EXTERNAL_IP=<host LAN/public IP>` in compose/env so coturn emits usable relay candidates.
   - Ensure all services use the same TURN URLs: `turn:${BEACH_TURN_EXTERNAL_IP}:3478` plus `turn:beach-coturn:3478` for in-cluster.
   - On the browser side (rewrite-2), add support to force relay for diagnostics (`iceTransportPolicy: "relay"` when a flag like `NEXT_PUBLIC_FORCE_TURN=1` is set).

2) **Controller forwarder readiness**
   - In manager controller forwarder, treat RTC as ready only after the action channel is open and ACKs observed; avoid preemptive fallback while channels are connected.
   - Add retry/holdoff before switching to HTTP if RTC is still connected and sending/receiving frames.

3) **Agent/browser action flow**
   - Verify agent/player are emitting actions; if empty, avoid declaring RTC unhealthy. Consider a heartbeat on the action channel to keep NAT bindings alive and to prove liveness.

## Logging to add (targeted and minimally noisy)
### Manager (beach-manager)
- On controller forwarder:
  - Log transitions of `rtc_ready` with reason, session_id, peer_session_id, transport_id, last_rx/tx timestamps, buffered_amount, and channel state (open/closing/closed).
  - Log when switching to HTTP with the trigger (timeout? missing ACK? channel closed?).
  - Log selected ICE pair (local/remote candidate, type, protocol) when RTC becomes connected.
- On signaling/ICE:
  - Log ICE servers actually passed to peers (count, URLs) per session_id.
  - Log datachannel close/error events (peer_id, label, code/reason).

### Host (beach-host-*)
- On 409/answer fetch: log peer_session_id, handshake_id, retry count, then log when a fetch succeeds (to correlate delay).
- Add ICE/connection state change logging (pion hooks) per peer_id/label.
- Log datachannel close/error events (peer_id, label, code/reason, bytes sent/received).

### Browser (rewrite-2)
- Log pc.onconnectionstatechange/oniceconnectionstatechange and datachannel close/error with session_id + peer_id.
- Log when connector switches transport with the condition (ack stall, channel close, explicit fallback).
- Log `iceServers` received for each tile and selected candidate types (host/srflx/relay).

## Experiments to run after fixes
1) With `BEACH_TURN_EXTERNAL_IP` set and `NEXT_PUBLIC_FORCE_TURN=1`, re-run and confirm relay candidates appear; check if RTC stays up >5 minutes.
2) With VPN/WARP off (removes 172.64.150.54 srflx), re-run to see if browser drop disappears.
3) Validate controller actions: add a simple ping/ack on the action channel to confirm liveness and keep NAT bindings active.

## Expected outcome
- Relay candidates available; browser↔host remains connected (or cleanly reports why not).
- Manager keeps controller forwarder on RTC when channels are open and ACKing; UI no longer shows false `http_fallback`.
- If stalls persist, new logs will pinpoint whether the failure is RTC transport, controller forwarding, or action source emptiness.

Feedback (Codex): Please also capture ICE servers and selected candidate pair at RTC ready/not-ready transitions, plus stall snapshots (inflight_by_transport, last_ack transport/age, channel state) when switching transports. A heartbeat action will help distinguish idle from stalled. No VETO.

## Updated plan (host↔manager vs browser↔host)
- Host↔manager is intra-Docker; TURN is **not** needed there. Focus on controller/action readiness and stall handling for that path.
- Browser↔host may still benefit from a correct TURN external IP and optional relay forcing to avoid srflx/hairpin fragility, but that is secondary to fixing controller forwarder logic.

### Fixes (incremental, implement in order)
1) **Controller forwarder readiness/stall logic**
   - Only mark RTC ready when the action channel is open and ACKs observed; add a heartbeat action (tiny no-op) every few seconds when idle.
   - When declaring stall/fallback, log snapshot: inflight_by_transport, last_ack transport/age, datachannel state, selected ICE pair.
   - Debounce stalls (e.g., 3–5s) and allow re-promotion to RTC once ACKs resume.
2) **Logging**
   - Manager: log ICE servers and selected candidate pair when RTC ready/not-ready; log datachannel close/error and controller action send/ack (seq, transport, inflight_before/after).
   - Host: log ICE/connection state changes and datachannel close/error for manager peers.
   - Browser: log connection state and transport switch reasons; log `iceServers` received.
3) **TURN (browser-only diagnostic)**
   - Set `BEACH_TURN_EXTERNAL_IP=<host LAN/public>` so coturn emits relay; add `NEXT_PUBLIC_FORCE_TURN=1` flag in rewrite-2 for a relay-only diagnostic run.

## Smoke test to validate host↔manager RTC and pong gameplay (no browser dependency)
Goal: restart stack, launch pong stack exactly like manual workflow, and assert controller path stays RTC and ball/paddle state progresses.

Steps (manual or scripted):
1) `./scripts/dockerup` (resets stack; matches user flow).
2) Create or reuse a beach ID (can create via API/CLI or reuse known ID).
3) Start the stack in manager container:  
   ```
   RUST_LOG=info,webrtc=trace,webrtc::ice_transport=trace,webrtc::peer_connection=trace \
   ./apps/private-beach/demo/pong/tools/pong-stack.sh start <beach-id>
   ```
   (This is the same command the user runs.)
4) Wait for sessions to bootstrap (lhs/rhs/agent) and for the tool to auto-install layout/attachments (same as manual).
5) Monitor ball/paddle trace files written every second by the players (already enabled via `PONG_BALL_TRACE_PATH` and `PONG_FRAME_DUMP_PATH`):
   - Files live in the manager container under `/tmp/pong-stack/<timestamp>/ball-trace/ball-trace-{lhs,rhs,agent}.jsonl` and `/tmp/pong-stack/<timestamp>/frame-dumps/frame-{lhs,rhs}.txt`.
6) Validation logic (can be a small Python script run via `docker exec beach-manager`):
   - Parse the three `ball-trace-*.jsonl` streams; ensure at any timestamp there is exactly one ball (same id) and its x/y positions advance across tiles (lhs↔rhs) over time.
   - Ensure paddle positions change over time (players/agent inputs present); if flat, flag controller/action stall.
   - Detect disconnects: if traces stop updating or the ball disappears, fail the test.
7) If validation passes for N minutes (e.g., 5–10), conclude host↔manager RTC and controller flow are healthy. If it fails, collect the logged snapshots (ICE pair, stall snapshot) to debug.

Notes for implementation:
- Reuse existing `pong-stack.sh` outputs; do not require a browser—ball/paddle traces come from the headless players.
- The validation script should run inside the manager container (has access to `/tmp/pong-stack/<timestamp>`). It can pick the latest run via `/tmp/pong-stack/latest`.
- Keep the smoke test idempotent: it should call `./scripts/dockerup`, run `pong-stack.sh start <id>`, run the validator, then exit non-zero on failure.

## Handover checklist for implementers
- Add controller heartbeat + stall debouncing/re-promotion in manager forwarder.
- Add the logging described above (manager, host, browser).
- Wire `BEACH_TURN_EXTERNAL_IP` in compose; add `NEXT_PUBLIC_FORCE_TURN` handling for rewrite-2 diagnostics (browser path only).
- Implement the Python validator for ball/paddle traces (single ball, movement across tiles, paddle movement) inside the manager container; wrap in a smoke test that runs `dockerup` + `pong-stack.sh start`.
- Run the smoke test; if it fails, capture logs/snapshots from the new instrumentation to iterate.

Feedback (Codex): Recommend adding per-action send/ack logs with transport + seq and a structured stall payload (inflight_by_transport, last_ack transport/age, stuck seqs) when declaring fallback; that’s how we’ll prove whether acks are missing vs miscounted. Also consider debouncing the stall detector and permitting re-promotion to RTC after acks flow again; the false stall we observed otherwise locks sessions on HTTP. No VETO.

Feedback (Codex agent 3): This “bridge” solution is compatible with the newer ack-stall analysis: TURN/env fixes stabilize browser RTC, and forwarder readiness rules keep manager on RTC when channels are genuinely healthy. I wouldn’t veto anything here; just make sure the controller’s stall decisions are backed by the richer ack/transport logs so we can validate the behavior in future runs.

Feedback (Codex agent 2): Agree. Add a guard so we fail fast if TURN URLs are set but `BEACH_TURN_EXTERNAL_IP` is missing—avoids silent srflx-only runs. Consider a brief RTC self-test (ping/echo) post-negotiation to assert rtc_ready, separating channel health from ack throughput. No VETO.

Feedback (Codex): Note manager↔host lives on the Docker bridge and doesn’t need TURN; TURN work here is for browser↔host. The false fallback we saw happened while manager RTC stayed up, so keep stall-detector tuning and per-action send/ack logging as the primary fixes to avoid downgrading a healthy RTC. No VETO.

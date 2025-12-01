# 2025-11-28 WebRTC fallback plan (bc4c51ba-2004-4e6a-9953-cbf31a83f0f6)

## Fixes to apply now
- **Enable valid TURN relay candidates everywhere**
  - Set `BEACH_TURN_EXTERNAL_IP=<host/LAN IP>` (e.g., `192.168.68.52`) in the env used by docker compose (`.env.local` or exported before `dockerup`).
  - Keep `BEACH_GATE_TURN_URLS=turn:<host IP>:3478` (no loopback); verify inside containers with `docker exec beach-manager env`.
  - Restart stack (`./scripts/dockerup`) so gate/coturn/manager/hosts all pick up the external IP and emit relay candidates.
- **Verify host port exposure**
  - Ensure UDP 62500–62600 and 3478 are published on the host and not blocked by firewall/pf. If in doubt, add a quick UDP check (e.g., netcat to 3478 from browser network) after restart.

## Strategic logging to add (next run)
- **ICE / candidate selection**
  - Host & manager: log applied ICE servers and the selected candidate pair (local/remote, type, protocol) when connection goes Connected/Completed.
  - Browser: capture webrtc-internals again to confirm relay candidates appear after TURN fix.
- **Controller forwarder health**
  - When `rtc_ready` flips or HTTP fallback is chosen, log peer_id/transport_id, data channel state, last sent/recv timestamps, buffered_amount, and any send errors. Include the trigger/reason string.
  - Log datachannel close/error events for manager peers on both host and manager.
- **Connector/agent clarity**
  - Make the UI log (and optionally surface) the active transport for actions/state (RTC vs HTTP) and the last ack latency per session, so “http_fallback” tiles are backed by state.

## “Crazy” edge checks (if issues persist)
- Check for VPN/WARP on the browser; disable to avoid short-lived srflx bindings.
- If still flapping after TURN fix, take a short tcpdump on host UDP 62500–62600 and 3478 to see if packets stop flowing (ISP/home router UDP filtering).
- Watch for mismatched publish URLs: manager logs show `BEACH_PUBLIC_SESSION_SERVER=http://192.168.68.52:4132`; ensure the browser can reach that IP from its network.

## Expected outcome
- With `BEACH_TURN_EXTERNAL_IP` set and relay candidates present, browser ↔ host should stay on RTC instead of dropping after ~1 minute. Manager should stop declaring `rtc_ready=false` if the controller channel remains healthy; if it still falls back, the added health logs will pinpoint whether the forwarder is mis-detecting channel state or suffering packet loss. 

Feedback (Codex): Good TURN/env and ICE logging. Please add explicit controller action/ack lifecycle logs (send vs ack, transport, seq, inflight counts) and include stall payload (inflight_by_transport, last_ack transport/age) when flipping to HTTP. The current plan doesn’t address the false ack-stall we saw; also consider a softer/debounced stall threshold or re-promotion back to RTC after acks resume.

Feedback (Codex agent 2): Agree—TURN external IP is the obvious missing piece. Add a guardrail: fail startup if `BEACH_TURN_EXTERNAL_IP` is unset while `BEACH_GATE_TURN_URLS` is set, to avoid silent hairpin-only runs. Also log selected ICE pair on manager/host to confirm we leave srflx-only paths. No VETO.

## Additional steps: end-to-end smoke for manager↔host RTC (no browser)
Goal: prove manager↔host WebRTC stays healthy under the same config as manual runs, and that the pong showcase state (ball/paddles) stays coherent (one ball, volleys across terminals).

### Smoke test outline (re-usable)
1) **Prep env**  
   - Export `BEACH_TURN_EXTERNAL_IP=<host IP>` and `BEACH_GATE_TURN_URLS=turn:<host IP>:3478` (same IP you’ll use for the browser).  
   - Ensure port range 62500–62600/udp and 3478/udp are free.
2) **Restart stack**  
   - `./scripts/dockerup` (matches the manual flow).
3) **Launch pong stack headless**  
   - `RUST_LOG=info,webrtc=trace,webrtc::ice_transport=trace,webrtc::peer_connection=trace ./apps/private-beach/demo/pong/tools/pong-stack.sh start <beach-id>`  
   - This mirrors the manual command you used.
4) **Monitor RTC health (manager↔host)**  
   - Tail `docker logs beach-manager` for session_id(s) to confirm `transport=webrtc`, no `rtc_ready=false`, and no fallbacks.  
   - Tail `/tmp/pong-stack/<timestamp>/beach-host-{lhs,rhs,agent}.log` for datachannel open/close and peer-left events; ensure manager peers do not leave.
5) **Monitor game state**  
   - The showcase writes a JSON snapshot of ball/paddle positions every second (file already present from prior change). Consume it (e.g., `jq` on the JSON stream) to assert:  
     - Exactly one ball object at any time.  
     - Ball X coordinate traverses between LHS/RHS over time (volleying).  
     - Paddles change positions (inputs flowing).  
   - If the JSON file is not yet wired, add a small script that polls the existing dump file (path: the new json trace under `/tmp/pong-stack/<timestamp>/` created by pong-stack) and runs these assertions.
6) **Pass/Fail criteria**  
   - RTC: Manager never flips to HTTP fallback; no `rtc_ready=false`; no manager peer left; no datachannel close for manager peers over a 5+ minute window.  
   - Game: JSON shows a single ball and coherent volley; no duplication or stuck ball; paddles move.

### If implementing the smoke test
- Add a script `apps/private-beach/demo/pong/tools/pong-smoke.sh` (or reuse an existing smoke harness if present) that:
  1) Runs `./scripts/dockerup`.
  2) Starts `pong-stack.sh start <beach-id>` with the RUST_LOG used above.
  3) Polls `docker logs beach-manager` and host logs for `rtc_ready=false`/fallback strings; fails if seen.
  4) Watches the JSON state dump for 300s: assert single ball, ball crosses midline at least N times, paddles update.
  5) Exits non-zero on failure; prints summary on success.
- Make the script parameterize `<beach-id>` and allow a `--duration` (default 300s).
- Keep it browser-free; only manager↔host RTC is exercised.

### Incremental implementation steps for an agent
1) Wire TURN guardrail: fail fast if `BEACH_TURN_EXTERNAL_IP` missing when TURN URLs set.  
2) Add ICE-pair logging on manager/host at Connected/Completed.  
3) Add forwarder rtc_ready/fallback logging with ack and channel state (per earlier feedback).  
4) Implement the `pong-smoke.sh` harness as above; reuse the existing JSON state dump to assert ball/paddle health.  
5) Run the smoke locally; if it fails, adjust logging to capture why (RTC drop vs ack stall vs game logic).  
6) Once passing, document how to run it in `docs/diagnosis/solutions/` or `docs/helpful-commands/`.

Additional feedback (Codex): Capture the actual ICE servers handed to browser/manager per session and the selected candidate pair (type/proto) at RTC ready/not-ready transitions. Add a simple controller heartbeat (no-op action) so forwarder has liveness before tripping stall logic, especially when user input is idle.

Feedback (Codex agent 3): I’d treat this as the “transport/env half” of the overall fix: necessary for browser↔host stability, but not sufficient to prevent the manager’s ack-stall logic from downgrading RTC while channels are fine—manager↔host is inside Docker and already has a healthy ICE/DC path. No VETO, but pair this plan with explicit changes to the controller stall detector (transport-aware thresholds, re-promotion) as captured in the ack-stall solution docs.

Feedback (Codex): Reminder that manager↔host runs inside Docker and does not need TURN; TURN fixes target browser↔host only. Ack-stall instrumentation and stall-detector tuning remain critical even if TURN is fixed, because we saw HTTP fallback while manager RTC stayed up.

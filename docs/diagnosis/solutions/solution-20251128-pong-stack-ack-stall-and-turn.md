# Solution plan: Pong stack WebRTC fallback (manager↔host ack stall + TURN/env) — 2025‑11‑28

This doc proposes concrete fixes and logging to address:

- Manager↔host controller fallback to HTTP despite healthy WebRTC (ack stall).
- Browser↔host WebRTC flakiness due to incomplete TURN/env.
- “Crazy” networking and race conditions called out in prior diagnoses.

The goal is to (a) make the system robust in the common case, and (b) ensure the next diagnostic run pinpoints any remaining edge cases unambiguously.

---

## 1. Code/config fixes

### 1.1 TURN / ICE configuration (browser ↔ host stability)

Problem:

- In the 2025‑11‑28 runs, `BEACH_GATE_TURN_URLS` was set to `turn:192.168.68.52:3478`, but `BEACH_TURN_EXTERNAL_IP` was **unset**.
- Coturn likely emitted no usable relay candidates; WebRTC peers (browser, manager, hosts) only saw `host` and `srflx` candidates.
- Browser↔host links succeeded via srflx/hairpin, then dropped ~1 minute later (binding expiry or NAT flakiness).

Proposed fix:

1. **Require `BEACH_TURN_EXTERNAL_IP` in dev and prod.**
   - On `dockerup` / `docker compose up`, compute or prompt for:
     - Dev: host LAN IP (e.g., `192.168.68.52`) if browser is on the same LAN.
     - Prod: a public IP or hostname reachable from browsers.
   - Propagate this into `coturn` via env and/or `turnserver.conf` `external-ip` directive.
2. Ensure all components share consistent ICE hints:
   - `BEACH_ICE_PUBLIC_IP` / `BEACH_ICE_PUBLIC_HOST` → same host IP as TURN for manager, road, hosts.
   - `BEACH_GATE_TURN_URLS` → `turn:<EXTERNAL_IP>:3478` for gate, manager, surfer, hosts (no `127.0.0.1`).
3. Add a startup sanity check:
   - If `BEACH_GATE_TURN_URLS` is set but `BEACH_TURN_EXTERNAL_IP` is missing, log a **WARNING** and optionally refuse to start in non‑dev; this config is almost always wrong.

Optional:

- Add a `NEXT_PUBLIC_FORCE_TURN=1` flag for surfer that forces browsers to prefer relay candidates when available, making TURN misconfigurations immediately obvious in `webrtc-internals`.

Important scoping note:

- Manager and hosts live on the same Docker network; their WebRTC path is via the Docker bridge and **does not require TURN**. The TURN/env fixes here are primarily to make the browser→host viewer connection robust. The manager↔host fallback we saw is a controller/ack logic issue, not a manager↔host ICE failure.

### 1.2 Controller forwarder ack‑stall behavior (manager ↔ host)

Problem:

- For LHS and RHS:
  - Manager↔host WebRTC channels negotiate successfully and stay in ICE `Connected`.
  - Host actively sends frames over WebRTC with `ready_state=Open` and moderate buffered amount.
  - Manager’s `controller.forwarder` later logs `ack stall detected; falling back to primary transport` with `transport="webrtc"` and flips pairing state to `HttpFallback`, even though the channels remain up.
  - Acks are being persisted, but `via_fast_path` vs `via_fast_path=false` is inconsistent, and many acks are counted as non‑fast‑path during the period claimed as “no acknowledgements”.
- The result: controller actions for players move permanently to HTTP fallback, while WebRTC is still healthy.

Proposed fixes:

1. **Make stall detection transport‑aware and monotonic.**
   - When computing “no action acknowledgements for >X ms”:
     - Track per‑transport ack times, e.g. `last_fastpath_ack_at`, `last_http_ack_at`.
     - Only declare a WebRTC stall if:
       - There are inflight WebRTC actions, **and**
       - `now - last_fastpath_ack_at > fastpath_stall_ms`, **and**
       - There is at least one send attempt recorded over WebRTC during that window.
   - If HTTP fallback acks are arriving (i.e., host is responsive), don’t immediately treat that as “no acknowledgements”; instead, downgrade with a specific reason (“fastpath ack gap; HTTP still healthy”).

2. **Require corroborating transport health before falling back.**
   - Before switching `child_session_id` pairing from `Rtc` to `HttpFallback`:
     - Check the current WebRTC transport health snapshot:
       - ICE state (Connected / Disconnected / Failed).
       - Datachannel state (Open / Closing / Closed).
       - Buffered amount (is it growing without draining?).
       - Recent send error counts.
     - Only fall back automatically if:
       - ICE / DC state is non‑healthy **or**
       - Buffered amount has exceeded a configured threshold for a sustained period **and**
       - No WebRTC acks are observed.
   - Otherwise:
       - Log a warning but keep WebRTC as primary, or
       - Switch to “mixed” mode (send some actions over HTTP, but keep WebRTC open) instead of fully demoting.

3. **Distinguish “no acks at all” from “no fast‑path acks”.**
   - Introduce distinct internal reasons:
     - `AckStallNone` — no acks on any transport.
     - `AckStallFastpathOnly` — HTTP acks arriving, WebRTC acks absent.
   - Only `AckStallNone` should trigger a hard failover to HTTP (and possibly session teardown); `AckStallFastpathOnly` should trigger a softer downgrade with aggressive logging.

4. **Debounce or cap fallback flapping.**
   - Once a child session has been downgraded to `HttpFallback` because of ack stall, either:
     - Stick with HTTP for the remainder of the session (unless explicitly re‑negotiated), or
     - Require an explicit positive signal to re‑promote to WebRTC (e.g., successful re‑negotiation plus a run of successful WebRTC acks).
   - This avoids rapid oscillation between states based on short‑term timing noise.

5. **Tune the stall timeout conservatively for dev; make it configurable.**
   - Default `fastpath_ack_stall_ms` to something like 5–10 seconds in dev, and only drop it in production if metrics justify it.
   - Expose an env var (e.g., `BEACH_FASTPATH_ACK_STALL_MS`) and log its value on startup.

### 1.3 Startup race between hosts and manager graph

Problem:

- Hosts attempted health/state publishes before their sessions existed; manager returned 404 “session not found”.
- Not catastrophic, but it introduces noise and may interact with the agent’s socket/health logic.

Proposed fix:

- Add a small backoff / retry window for initial health/state publishes:
  - On a 404, treat it as “graph not ready yet” and retry a few times with a short delay, logging at DEBUG (not WARN) if it resolves within a threshold (e.g., 5 seconds).
- Optionally, wait for an explicit manager handshake / attach confirmation before enabling health reporter on hosts, to minimize transient 404s.

---

## 2. Strategic logging and instrumentation

To confirm the fixes and catch any “crazy” issues, we should add structured logging in a few key areas.

### 2.1 Manager: controller action and ack lifecycle

Add structured logs for:

1. **Action send events:**
   - When `controller.forwarder` sends an action:
     - `action_send{session_id, private_beach_id, seq, action_id, transport, via_fast_path, peer_session_id, transport_id}`.

2. **Ack receipt / persistence:**
   - When an ack is observed and persisted by `controller.delivery`:
     - `action_ack{session_id, private_beach_id, seq, action_id, ack_from_transport, via_fast_path, inflight_before, inflight_after, ack_count_now, ms_since_action_sent}`.

3. **Stall detection:**
   - At the moment we currently log `ack stall detected`:
     - `ack_stall{session_id, private_beach_id, reason, inflight_by_transport, last_fastpath_ack_ms, last_http_ack_ms, actions_without_ack=[(seq, action_id, transport, ms_since_sent)...]}`.

4. **Pairing state transitions:**
   - In `update_pairing_transport_status`:
     - `pairing_update{child_session_id, previous_transport, new_transport, cause, error, transport_id, peer_session_id}`.

This will allow us to reconstruct exactly which actions were considered stuck, on which transport, and whether acks were actually missing vs misclassified.

### 2.2 Host: controller actions and acks

On each host (`beach-host-*`):

1. **Controller action receipt:**
   - Log when controller actions arrive:
     - `controller_action_recv{session_id, private_beach_id, seq, action_id, transport_source}`.

2. **Ack emission:**
   - Log when we send a controller ack:
     - `controller_ack_send{session_id, private_beach_id, seq, action_id, transport_used, peer_session_id, transport_id}`.

3. **Datachannel state and backpressure:**
   - Existing `dc.buffered_amount` logs are good; augment with:
     - `dc_health{session_id, peer_label, transport_id, ready_state, buffered_amount, last_send_ms, last_recv_ms}` at some interval, or when buffered amount crosses thresholds.
   - Log any `ICEConnectionState` transitions to `Disconnected` or `Failed` with:
     - `ice_state_change{session_id, peer_label, new_state, reason_if_known}`.

### 2.3 Manager + host: WebRTC and TURN visibility

1. **ICE servers and selected pair:**
   - On both manager and hosts, dump:
     - ICE server list (STUN/TURN URLs, credentials redacted).
     - The selected candidate pair details (local/remote, type: host/srflx/relay, protocol).
   - Log once per peer session (on connection or first `Connected` state).

2. **TURN configuration snapshot:**
   - At startup, log:
     - `turn_config{BEACH_TURN_EXTERNAL_IP, BEACH_GATE_TURN_URLS, realm, port}`.
   - If `BEACH_TURN_EXTERNAL_IP` is missing while TURN URLs are set, emit a prominent warning.

### 2.4 Browser: connector and viewer

In the frontend connector:

1. **Transport updates with cause:**
   - For `connector.transport:update`:
     - Include `old_transport`, `new_transport`, `reason`, `session_id`, `controller_session_id`.
     - Reasons: `ack_stall`, `webrtc_negotiation_failed`, `user_prefers_http`, `initial_state`, etc.

2. **Viewer WebRTC state:**
   - Log `pc.oniceconnectionstatechange` and `pc.onconnectionstatechange` with:
     - `viewer_rtc_state{session_id, peer_id, new_state, reason}`.
   - On datachannel `close`/`error`, log:
     - `viewer_dc_event{session_id, peer_id, label, event, code?, reason?}`.

This will help differentiate “connector fell back because manager told it to” from “viewer pc actually closed”.

### 2.5 Agent: crazy‑angle instrumentation

The agent already logs fallback decisions and `queue_action` results; we should:

1. Ensure each “allowing HTTP fallback” log includes:
   - `session_id`, `private_beach_id`, `child_role` (lhs/rhs), `reason`, and the manager’s reported `rtc_ready` and `selected_transport`.

2. For watchdog‑driven serves and timing‑sensitive behavior:
   - Log the controller `transport` that was active when decisions like “fast‑path unavailable; falling back” or “ball watchdog forced serve” occur.

This will catch weirder scenarios like:

- VPN/WARP interference (browser srflx from Cloudflare IP).
- Docker publish or host firewall misconfig causing intermittent packet loss.
- Agent reacting to stale manager health data.

---

## 3. Validation / experiment matrix

After implementing the fixes + logging, we should run a small matrix:

1. **Baseline (fixed TURN, unchanged stall threshold):**
   - `BEACH_TURN_EXTERNAL_IP` set to LAN IP, all services restarted via `dockerup`.
   - Run `pong-stack.sh` once with browser on the same LAN, WARP/VPN **off**.
   - Expectation:
     - Browser↔host uses relay or stable srflx; viewer WebRTC stays up >5 minutes.
     - Manager↔host WebRTC stays `Rtc`; no `AckStall` for player sessions.

2. **Stress ack‑stall logic:**
   - Artificially delay acks on host (e.g., instrument a sleep in ack path) to trigger `AckStallFastpathOnly` and verify:
     - Manager logs show the new structured `ack_stall` reason, and
     - Pairing transitions behave as designed (soft downgrade vs hard failover).

3. **VPN/WARP / off‑LAN scenario:**
   - Run once with browser on a different network or behind WARP/VPN.
   - Confirm that:
     - Relay candidates appear and are selected.
     - If they don’t, the logging clearly implicates TURN reachability / firewall issues, not the controller pipeline.

---

## 4. Summary

Short‑term:

- Fix TURN/env so relay candidates exist and are used when needed.
- Harden the controller forwarder’s ack stall detection so it doesn’t prematurely abandon a healthy WebRTC path.
- Add targeted logging on both manager and hosts to trace action/ack lifecycles and transport health.

Long‑term:

- Use the new instrumentation plus a few controlled experiments to prove or disprove the remaining hypotheses (ack misrouting vs misclassification vs genuine network stalls) and evolve the fallback logic accordingly.

Feedback (Codex): Strong coverage of TURN/env and stall logic. Please ensure the proposed logs explicitly correlate action send → ack by transport (seq/action_id/inflight_before/after) and include stall snapshots (inflight_by_transport, last_ack_transport/age, candidate stuck seqs). Also consider lowering aggressiveness: debounce the stall detector or allow re-promotion to WebRTC once acks resume; the current stickiness made the session permanently HTTP even while WebRTC stayed connected.

Feedback (Codex agent 3): No VETO here—this solution best reflects the latest evidence. The only refinement I’d add is to treat `BEACH_TURN_EXTERNAL_IP`/TURN correctness as necessary for browser stability but not as the fix for manager↔host fallback; manager and hosts live on the same Docker network and already negotiate ICE/DC successfully, so that part is squarely the controller ack-stall logic this doc already targets.

Additional feedback (Codex): Add logging of the ICE servers actually applied and the selected candidate pair when the forwarder marks rtc_ready true/false. If possible, inject a heartbeat action so idle sessions don't trip stalls. No VETO—plan is sound with these instrumentation tweaks.

Feedback (Codex agent 2): Agree. Add a startup guardrail: fail fast if TURN URLs are set but `BEACH_TURN_EXTERNAL_IP` is absent, to avoid silent hairpin-only runs. Consider a post-negotiation self-test (RTC ping/echo) to assert rtc_ready rather than inferring from acks alone; would help separate channel liveliness from ack bookkeeping. No VETO.

Feedback (Codex): Reminder that manager↔host is on the Docker bridge and not TURN-dependent; TURN helps browser↔host only. The controller ack-stall tuning remains the key fix for the HTTP downgrade while RTC stayed up, so keep the per-action send/ack + stall snapshots at the top of the priority list. No VETO.

---

## 5. Manager↔host WebRTC smoke test (no browser)

This smoke test is meant to exercise the full Pong stack (including the agent and both paddles) and validate manager↔host WebRTC stability and showcase behavior **without opening a browser**. It should be implemented as a small harness that:

- Restarts the stack with `./scripts/dockerup`.
- Uses `pong-stack.sh` to start LHS, RHS, and agent and configure the beach layout exactly as in the manual flow.
- Monitors the ball trace JSONL files to ensure a single ball is present at any time and that it volleys across both terminals.

### 5.1 High-level flow

1. From repo root, run `./scripts/dockerup` to restart the full stack with the usual dev env (including `BEACH_ICE_PUBLIC_IP/HOST` and TURN settings).
2. Invoke `apps/private-beach/demo/pong/tools/pong-stack.sh` in the **same way** you do manually, but from a script:
   - If you want to reuse an existing beach: `pong-stack.sh --setup-beach start <private-beach-id>`.
   - If you want the test to create a fresh beach each time: `pong-stack.sh --create-beach --setup-beach start create-beach`.
   - Set `PONG_LOG_DIR` (and optionally `PONG_LOG_ROOT`) so the test knows exactly where to read logs/trace files.
3. Wait for bootstrap to complete:
   - `pong-stack.sh` already waits for LHS, RHS, and agent bootstrap and wires the session-graph via `--setup-beach`. The smoke test just needs to wait until `pong-stack.sh` returns success.
4. Once the stack is up and the showcase is configured, monitor the ball trace JSONL files produced by the players/agent to:
   - Assert there is at most one active ball at any sampled time.
   - Assert that over time the ball’s X coordinate moves back and forth between the LHS and RHS sides (i.e., across terminals) rather than getting “stuck”.
5. Run this for a fixed duration (e.g., 2–5 minutes) and fail if any of the invariants are violated or if manager↔host RTC shows a stall.

### 5.2 Reusing existing tooling

You already have:

- `apps/private-beach/demo/pong/tools/pong-stack.sh` — orchestrates LHS/RHS/agent, performs `--setup-beach`, and writes logs/trace files under `/tmp/pong-stack` (inside the manager container) according to:
  - `PONG_BALL_TRACE_DIR` (default `$LOG_DIR/ball-trace`).
  - `PONG_STATE_TRACE_DIR` (default `$LOG_DIR/state-trace`).
  - `PONG_STATE_TRACE_INTERVAL` (seconds between state snapshots).
- `apps/private-beach/demo/pong/tools/smoke_test.py` — a Python smoke test that hits the Manager API and validates session metadata and controller pairings (agents vs paddles).

To avoid duplicating logic, the new smoke test can:

1. Reuse `smoke_test.py` to confirm:
   - The correct sessions (lhs, rhs, agent) are attached.
   - Controller pairings exist from the agent to both paddles.
2. Add a **second phase** that inspects the ball trace JSONL files to check actual gameplay behavior and implicitly manager↔host WebRTC stability.

### 5.3 Proposed smoke test script shape

Create a new script under `apps/private-beach/demo/pong/tools/`, e.g. `pong_rtc_smoke.py`, with roughly this behavior:

1. **CLI arguments / env:**
   - `--manager-url` (e.g., `http://127.0.0.1:8080` for host, or `http://localhost:8080` inside container).
   - `--private-beach-id` (optional if using `--create-beach`; otherwise required).
   - `--auth-token` (or use `PB_MANAGER_TOKEN`).
   - Optional duration (`--duration-seconds`, default ~180).
   - Optional `--log-dir` (default `/tmp/pong-stack/latest` inside the container).

2. **Phase 1: restart stack + launch showcase**
   - From the host, invoke:
     - `subprocess.run(["./scripts/dockerup"], check=True)` to restart the stack.
   - Then invoke `pong-stack.sh` inside `beach-manager` using `docker compose exec`:
     - For a known beach:
       - `docker compose exec beach-manager bash -lc 'cd /app && PONG_LOG_DIR=/tmp/pong-stack/smoke LOG_ROOT=/tmp/pong-stack ./apps/private-beach/demo/pong/tools/pong-stack.sh --setup-beach start <id>'`
     - For auto-create:
       - Same, but `pong-stack.sh --create-beach --setup-beach start create-beach`, and read the created beach ID from `PONG_CREATED_BEACH_ID_FILE` or from the script output.
   - Wait for this command to return; on success, LHS/RHS/agent are up and wired, with logs in `$LOG_DIR`.

3. **Phase 2: reuse `smoke_test.py` for structural checks**
   - From the host, call:
     - `docker compose exec beach-manager bash -lc 'cd /app && python3 apps/private-beach/demo/pong/tools/smoke_test.py --manager-url "$PRIVATE_BEACH_MANAGER_URL" --private-beach-id "<id>"'`
   - Fail the smoke test if this script returns non-zero (missing sessions, pairings, etc.).

4. **Phase 3: ball trace invariants (behavior + RTC stability)**

Within the manager container (or by copying the files out), inspect:

- `PONG_BALL_TRACE_DIR`:
  - Typically `/tmp/pong-stack/.../ball-trace/ball-trace-lhs.jsonl` and `ball-trace-rhs.jsonl` as configured in player/env.
  - Each line is a JSON object recorded every `PONG_STATE_TRACE_INTERVAL` seconds with at least:
    - A timestamp.
    - Ball position (x,y).
    - Possibly which side/terminal is active.

Implementation outline (in `pong_rtc_smoke.py` or a separate helper):

1. Sample for `duration_seconds` (e.g., 180s), reading new lines from both ball trace files.
2. At each timestamp:
   - Aggregate all ball entries from both files whose timestamps fall within a small window (e.g., ±0.5s).
   - Assert:
     - There is **at most one ball** (one active logical ball) across both files for that window.
       - If the schema has an explicit ball ID, ensure only one unique ball ID appears.
       - If not, treat any non-`null` ball entries as a ball and check `len(non_null_balls) <= 1`.
3. Over the whole run:
   - Track the ball’s X coordinate and whether it is associated with the left or right paddle/terminal.
   - Assert that:
     - There is at least one volley where the ball moves from “left side” (low X) to “right side” (high X) and back, indicating cross-terminal movement.
     - The ball does not “freeze” on one side for the entire duration.
4. Optionally, correlate periods where:
   - Manager logs `ack stall detected` or pairing flips to `HttpFallback`.
   - With the ball trace:
     - If the ball stops moving around the same time, that’s a strong signal that controller actions are no longer flowing.
   - The smoke test can fail if:
     - Any ack stall is logged for the LHS/RHS sessions during the run, **or**
     - The ball freezes (no X movement) for longer than some threshold (e.g., 10–15 seconds).

### 5.4 Implementation steps for another agent

To make this easy to execute incrementally, another agent could:

1. **Add the smoke test harness:**
   - Create `apps/private-beach/demo/pong/tools/pong_rtc_smoke.py` with the CLI + three phases described above.
   - Wire it into a simple host-side wrapper script (e.g., `scripts/pong_rtc_smoke.sh`) that:
     - Runs `./scripts/dockerup`.
     - Calls `docker compose exec beach-manager python3 /app/apps/private-beach/demo/pong/tools/pong_rtc_smoke.py ...`.

2. **Implement ball trace parsing:**
   - Inside `pong_rtc_smoke.py`, open the ball trace files under `$LOG_DIR/ball-trace/` (or accept a `--ball-trace-dir` flag).
   - Implement the single-ball and cross-terminal movement checks as described.

3. **Hook in manager log checks:**
   - Optionally, tail `/var/log/beach-manager/beach-manager.log` (or a copy under `logs/`) to detect any `ack stall detected` or `new_transport=HttpFallback` for the test sessions during the run and fail if seen.

4. **Run and iterate:**
   - Run the smoke test locally; if it fails due to ball invariants or ack stalls:
     - Use the already-proposed logging and controller changes to debug.
   - Once stable, integrate this smoke test into your CI or a manual regression checklist for changes touching WebRTC or controller forwarding.

Additional feedback (Codex): Add logging of the ICE servers actually applied and the selected candidate pair when the forwarder marks rtc_ready true/false. If possible, inject a heartbeat action so idle sessions don't trip stalls. No VETO—plan is sound with these instrumentation tweaks.

Feedback (Codex agent 2): Agree. Add a startup guardrail: fail fast if TURN URLs are set but `BEACH_TURN_EXTERNAL_IP` is absent, to avoid silent hairpin-only runs. Consider a post-negotiation self-test (RTC ping/echo) to assert rtc_ready rather than inferring from acks alone; would help separate channel liveliness from ack bookkeeping. No VETO.

Feedback (Codex): Reminder that manager↔host is on the Docker bridge and not TURN-dependent; TURN helps browser↔host only. The controller ack-stall tuning remains the key fix for the HTTP downgrade while RTC stayed up, so keep the per-action send/ack + stall snapshots at the top of the priority list. No VETO.

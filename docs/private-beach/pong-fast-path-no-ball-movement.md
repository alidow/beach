# Pong Fast-Path: No Ball Movement (Status + Handoff)

## Goal

Make the Pong showcase a realistic fast-path regression test: both paddles connected via controller fast-path, with an agent spawning a ball that travels across the screen and is hit at least once by each player. The test should be fully headless (no browser), runnable in CI, and driven by logs / traces.

As of the latest run:

- Controller fast-path is stable (no churn, no HTTP fallback).
- The end-to-end harness passes its fast-path stability checks.
- Pong appears idle: there is no observed ball movement in the captured frames or traces.

This doc captures the current problem, fixes already applied, and how to run + interpret the end-to-end test so another agent can pick up the investigation.

---

## Current Symptom

On a recent run:

```bash
FASTPATH_KEEP_STACK=1 direnv exec . python3 scripts/fastpath-integration.py
```

produced:

- `temp/fastpath-integration/latest-run.log` ending with:

  ```text
  [fastpath] PASS – fast-path stayed stable for sessions:
    lhs: c5bbaac0-0ec3-457e-9273-78e3ad3f039c
    rhs: d2d4ec53-ea6e-4ab2-ac0c-56e60d1e37bb
    agent: 9a66a501-2403-4710-9882-25583245537d
  ```

- But the gameplay traces show no ball:

  - `temp/fastpath-integration/ball-trace-lhs.jsonl` – empty.
  - `temp/fastpath-integration/ball-trace-rhs.jsonl` – empty.
  - All sampled frame dumps look like:

    - `lhs-frame-*.txt` contain:

      ```text
      ● Ready. · Mode LHS @ X3
      ```

    - `rhs-frame-*.txt` contain:

      ```text
      ● Ready. · Mode RHS @ X76
      ```

    - That HUD line (with the status dot, not the ball) is identical across all sampled frames; there is no `●` glyph moving within the playfield.

  - `temp/fastpath-integration/command-trace-lhs.log` and `command-trace-rhs.log` are empty.

Interpretation: the test harness now proves fast-path channel stability, but no ball ever appears or moves on either player view. Either the ball-spawn command never reaches the players, or it is accepted but not rendered / traced.

---

## How the End-to-End Test Works

The harness lives in:

- `scripts/fastpath-integration.py`
- `apps/private-beach/demo/pong/tools/pong-stack.sh`

### Environment Prereqs

- Docker Desktop running.
- `direnv allow` at repo root so `.envrc` can set `BEACH_ICE_PUBLIC_IP/HOST`.
- Env vars (usually via `.envrc` / `.env`):
  - `BEACH_ICE_PUBLIC_IP` / `BEACH_ICE_PUBLIC_HOST` – LAN IP used for WebRTC NAT hints.
  - `PRIVATE_BEACH_MANAGER_TOKEN` – dev-only token for manager API.
- Optional but recommended when debugging:
  - `FASTPATH_KEEP_STACK=1` – avoid tearing down containers when the run fails.

### Harness Execution Flow

1. **Ensure ports are free**

   `fastpath-integration.py` frees:

   - UDP: `62000–62100` – ICE ports used by manager’s Pion stack.
   - TCP: `8080` (manager), `4132` (beach-road), `4133` (beach-gate), `5173` (cabana dev-server).

2. **Start docker-compose stack**

   The script runs:

   ```bash
   direnv exec . docker compose up -d \
     postgres redis coturn beach-gate beach-road beach-manager db-migrate db-seed
   ```

   It then waits for:

   - Postgres to be healthy.
   - DB migrations / seed to complete.
   - `beach-manager` to be up and answering on `http://127.0.0.1:8080`.

3. **Launch Pong stack inside manager container**

   Uses:

   - `apps/private-beach/demo/pong/tools/pong-stack.sh start <private-beach-id>`

   This:

   - Ensures `beach` CLI is logged in (mock Clerk).
   - Starts:
     - LHS player host (`cargo run --bin beach ... host -- .../player/main.py --mode lhs`)
     - RHS player host (`... --mode rhs`)
     - Pong agent (`apps/private-beach/demo/pong/tools/run-agent.sh`).
   - Writes logs under `/tmp/pong-stack` inside the container:
     - `beach-host-{lhs,rhs,agent}.log`
     - `player-{lhs,rhs}.log`
     - `agent.log`
   - Prints:
     - `role session_id passcode` for `lhs`, `rhs`, `agent`.

   Env used by `pong-stack.sh`:

   - `PONG_FRAME_DUMP_DIR=/tmp/pong-stack/frame-dumps`
   - `PONG_BALL_TRACE_DIR=/tmp/pong-stack/ball-trace`
   - `PONG_COMMAND_TRACE_DIR=/tmp/pong-stack/command-trace`

   Those are picked up by `player/main.py` to:

   - Dump periodic frame snapshots (`frame-lhs.txt`, `frame-rhs.txt`).
   - Log ball positions to `ball-trace-*.jsonl`.
   - Log received commands to `command-*.log`.

4. **Attach sessions & acquire controller leases**

   The harness uses the manager API (`MANAGER_BASE_URL`, `PRIVATE_BEACH_MANAGER_TOKEN`) to:

   - Create a temporary private beach.
   - Attach the `lhs`, `rhs`, and `agent` sessions by passcode:
     - `POST /private-beaches/{id}/attach-by-code`
   - Acquire controller leases for the agent:
     - `POST /sessions/{agent_session_id}/controller/lease?ttl_ms=...`

5. **Wait for fast-path stabilization**

   Harness waits ~25 seconds (configurable via `FASTPATH_COMMAND_READY_WAIT`) so:

   - Hosts negotiate WebRTC via beach-road.
   - Manager creates `FastPathSession` per controller session via:
     - `POST /fastpath/sessions/:id/webrtc/offer`
     - `POST /fastpath/sessions/:id/webrtc/ice`
   - `mgr-actions` data channel goes online, HTTP pollers pause.

6. **Spawn ball & capture frames**

   - Harness enqueues a small set of controller actions (including the ball-spawn command) via:
     - `POST /sessions/{lhs,rhs}/actions` with an appropriate `controller_token`.
   - It then:
     - Sleeps a bit (`FASTPATH_FRAME_WARMUP`).
     - Samples frames `FRAME_SAMPLE_COUNT` times, copying:
       - `/tmp/pong-stack/frame-dumps/frame-lhs.txt`
       - `/tmp/pong-stack/frame-dumps/frame-rhs.txt`
       to `temp/fastpath-integration/lhs-frame-{i}.txt` and `rhs-frame-{i}.txt`.
     - Copies:
       - `/tmp/pong-stack/ball-trace/ball-trace-{lhs,rhs}.jsonl`
       - `/tmp/pong-stack/command-trace/command-{lhs,rhs}.log`
       into `temp/fastpath-integration/`.

7. **Analyze logs & assert**

   The script inspects:

   - `manager.log` – exported from `/var/log/beach-manager/beach-manager.log`.
   - `beach-host-{lhs,rhs,agent}.log` – host logs.
   - `agent.log`, player logs.
   - Frame/ball/command traces.

   It asserts:

   - Fast-path channel comes up and stays active (no repeated fallbacks).
   - No controller data-channel timeouts / 404 storms.
   - At least minimal action throughput.
   - (Future work) Ball makes it across the playfield and is hit at least once by each paddle.

On the latest run, all fast-path stability checks passed; only the ball-motion checks remain unimplemented/failing.

---

## Fixes Already Applied (Fast-Path Side)

### 1. Manager fast-path send semantics

Files:

- `apps/beach-manager/src/fastpath.rs`
- `apps/beach-manager/src/state.rs`

Changes:

- `FastPathSession` now:
  - Creates its own Pion `RTCPeerConnection` per session with:
    - NAT 1:1 hints from `BEACH_ICE_PUBLIC_IP/HOST`.
    - UDP port range from `BEACH_ICE_PORT_START/END`.
  - Records data channels:
    - `mgr-actions`, `mgr-acks`, `mgr-state`.
  - Exposes `instance_id` and a monotonically increasing `seq` counter.

- `send_actions_over_fast_path`:
  - Looks up `FastPathSession` from `FastPathRegistry`.
  - For each `ActionCommand`:
    - Extracts raw terminal bytes via `fast_path_action_bytes` (only `terminal_write` supported).
    - Wraps bytes into `WireClientFrame::Input { seq, data }`.
    - Encodes via `protocol::encode_client_frame_binary`.
    - Sends bytes over the `mgr-actions` `RTCDataChannel` with a timeout.
  - Logs:
    - `sending actions over fast-path channel` and `fast-path actions delivered` with `fast_path_id`.

### 2. Manager queue_actions fast-path gating

File:

- `apps/beach-manager/src/state.rs:queue_actions`

Original bug:

- When `transport_mode == TransportMode::FastPath` and `fast_path_ready == true`:
  - Manager would call `send_actions_over_fast_path`.
  - On `FastPathSendOutcome::Delivered`, it:
    - Updated pairing status to `fast_path`.
    - Logged `"dispatched actions via fast-path"`.
    - **Returned early**, never enqueueing the same actions into Redis.
- `drive_controller_forwarder` only pulls from `poll_actions` (Redis/HTTP). It knew nothing about `FastPathSession`.
- Result: controller forwarder never saw any actions to send into the host’s `run_fast_path_controller_channel`, even though manager logged fast-path delivery.

Fix:

- Introduced `fast_path_delivered: bool` in `queue_actions`.
- On `FastPathSendOutcome::Delivered`:
  - Kept all existing behavior (metrics, pairing status, events).
  - Set `fast_path_delivered = true`.
  - **Did not return**; allowed logic to fall through into the Redis enqueue branch.
- When deciding whether to reject commands for FastPath-only sessions:

  ```rust
  if matches!(transport_mode, TransportMode::FastPath) && !fast_path_delivered {
      // still reject with FastPathNotReady
  }
  ```

- This means:
  - Fast-path delivery and Redis enqueue both happen for FastPath sessions.
  - `poll_actions` now returns actions even when fast-path is preferred, so controller forwarder can feed the host’s fast-path controller channel.

Status:

- All fast-path related unit tests in `beach-manager` pass:

  ```bash
  cargo test -p beach-manager fast_path
  ```

### 3. Harness fast-path decode (beach-buggy)

File:

- `crates/beach-buggy/src/fast_path.rs`

Original bug:

- `FastPathClient::wire_action_handler` assumed:
  - `msg.is_string == true`.
  - Payload is chunked JSON (`{"type":"chunk","scope":"actions",...}` → `ActionCommand`).
- After the manager switched to binary `WireClientFrame::Input` on `mgr-actions`, the client saw `msg.is_string == false` and logged:

  ```text
  WARN beach_buggy::fast_path: failed to decode chunked action message ... expected text payload for action message
  ```

Fix:

- `wire_action_handler` now:
  - If `msg.is_string`:
    - Keeps the existing chunked JSON path (`decode_fast_path_payload` + `parse_action_payload`).
  - Else:
    - Calls a new `decode_binary_action_message` helper that:
      - Reads the binary protocol header (version/type bits).
      - Validates the frame is a client input frame.
      - Reads `seq` and `len` varints, then the `len`-byte payload.
      - Interprets payload bytes as UTF-8 text and builds an `ActionCommand`:

        ```rust
        ActionCommand {
            id: format!("fastpath-seq-{seq}"),
            action_type: "terminal_write".into(),
            payload: json!({ "bytes": text }),
            expires_at: None,
        }
        ```

    - Broadcasts that `ActionCommand` via the existing `actions` channel.

Status:

- All fast-path tests in `beach-buggy` pass:

  ```bash
  cargo test -p beach-buggy fast_path
  ```

---

## What’s Left: Ball Movement

We now have:

- Stable fast-path channels (no HTTP fallbacks).
- Manager delivering actions over both fast-path (`FastPathSession`) and Redis/forwarder.
- Host receiving controller frames over `run_fast_path_controller_channel` (verified in earlier logs; not re-dumped here).
- Harness that can prove fast-path stability via `scripts/fastpath-integration.py`.

But:

- Frame dumps show only the static HUD line (`● Ready. · Mode {LHS,RHS}`).
- Ball traces (`ball-trace-*.jsonl`) are empty.
- Command traces (`command-trace-*.log`) are empty.

Likely remaining issues:

- The ball-spawn command (`b <y> <dx> <dy>`) is:
  - Either not being queued (HTTP/fast-path mismatch, wrong session/lease, or 412 precondition failures).
  - Or being queued but not written to the players’ PTYs.
  - Or being written, but the player TUI’s ball logic isn’t enabled/seeing it (e.g., command parsing, mode gating).

Key code paths to inspect next:

- Harness command injection:
  - `scripts/fastpath-integration.py` – where it calls `queue_actions` to send `terminal_write` actions that encode pong commands (`m`, `b`, etc).
  - `apps/private-beach/demo/pong/tools/run-agent.sh` – how the agent decides when to enqueue rally commands.

- Manager delivery:
  - `apps/beach-manager/src/state.rs` – `queue_actions` path for the Pong sessions (check logs for `trace_id` markers used by the harness).
  - `apps/beach-manager/src/state.rs` – controller forwarder logs:
    - `controller.delivery` (forwarded actions to host).

- Host/PTTY:
  - `apps/beach/src/server/terminal/host.rs` – `run_fast_path_controller_channel` and HTTP fallback:
    - Confirm `applied fast path controller bytes` logs align with Pong commands.
  - `apps/private-beach/demo/pong/player/main.py` – command parser:
    - `m <delta>`; `b <y> <dx> <dy>`.
    - Ball integration with frame dumps (`PONG_FRAME_DUMP_PATH`) and ball traces (`PONG_BALL_TRACE_PATH`).

Log files to study for ball issues:

- `temp/fastpath-integration/manager.log`
- `temp/fastpath-integration/beach-host-{lhs,rhs}.log`
- `temp/fastpath-integration/player-{lhs,rhs}.log`
- `temp/fastpath-integration/agent.log`
- `temp/fastpath-integration/command-trace-{lhs,rhs}.log`
- `temp/fastpath-integration/ball-trace-{lhs,rhs}.jsonl`

---

## How to Pick Up From Here

For another agent stepping in with minimal context:

1. **Run the end-to-end harness (from repo root)**

   ```bash
   direnv allow                     # one-time, if not already done
   FASTPATH_KEEP_STACK=1 direnv exec . python3 scripts/fastpath-integration.py
   ```

   - If Docker is flakey:
     - First verify: `direnv exec . docker ps` succeeds.
     - If not, restart Docker Desktop and retry.

2. **Check high-level result**

   - Open `temp/fastpath-integration/latest-run.log`.
   - Confirm it ends with `PASS – fast-path stayed stable for sessions: ...`.

3. **Inspect gameplay traces**

   - Frames:

     - `temp/fastpath-integration/lhs-frame-*.txt`
     - `temp/fastpath-integration/rhs-frame-*.txt`

     Look for:

     - Ball glyph `●` moving across columns/rows over time.
     - Not just the static HUD status dot.

   - Command traces:

     - `temp/fastpath-integration/command-trace-lhs.log`
     - `temp/fastpath-integration/command-trace-rhs.log`

     Expect to see lines for `m` and `b` commands.

   - Ball traces:

     - `temp/fastpath-integration/ball-trace-lhs.jsonl`
     - `temp/fastpath-integration/ball-trace-rhs.jsonl`

     Expect JSON lines with `(x,y)` or similar positions sampled over time.

4. **If frames/traces still show no ball**

   - Confirm that the harness is actually queuing the ball-spawn action:
     - Search `manager.log` for the harness `trace_id` and the ball command payload.
   - Confirm host is applying those bytes:
     - Look for `controller.actions.fast_path.apply` logs in `beach-host-*.log` around the same time.
   - If both show the command applied, focus on:
     - `player/main.py` parsing logic and whether it logs ball traces when `PONG_BALL_TRACE_PATH` is set.

5. **When making changes**

   - Keep test loop:

     ```bash
     cargo test -p beach-buggy fast_path
     cargo test -p beach-manager fast_path
     FASTPATH_KEEP_STACK=1 direnv exec . python3 scripts/fastpath-integration.py
     ```

   - Use `temp/fastpath-integration` as your primary debugging workspace.

---

## TL;DR for the Next Agent

- Fast-path data channel plumbing between manager and host is now working and stable under the harness.
- The remaining bug is purely gameplay-level: the ball never appears/moves in the Pong showcase despite the controller path being up.
- Focus next on how the ball-spawn commands are queued, delivered, and interpreted by the Python player TUI and how those are reflected in `command-trace-*.log` and `ball-trace-*.jsonl`. This doc plus `scripts/fastpath-integration.py` and `temp/fastpath-integration/*` should give you everything you need to continue. 


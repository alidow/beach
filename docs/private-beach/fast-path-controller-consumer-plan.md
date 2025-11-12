# Fast Path Controller Consumer Implementation Plan

## Goal
Public Beach hosts must consume controller actions delivered over the fast_path `pb-controller` channel so Pong (and future controller-driven apps) run without any manual environment variables. We keep the existing HTTP poll/ack flow as a fallback, but prioritize the lower-latency fast_path delivery.

## Background & Current State
- Beach Manager already mirrors controller actions onto two transports:
  1. **HTTP queue** (`/sessions/:id/actions/poll` + `/ack`). Hosts poll this today; it works but adds latency and requires hosts to attach explicitly.
  2. **fast_path forwarder** (`pb-controller` data channel). Manager pushes every controller frame to Beach Road, which forwards it to the host over WebRTC. This is how Private Beach rewrite-2 tiles stream controller actions now.
- CLI hosts (`apps/beach/src/server/terminal/host.rs`) only know how to:
  - Poll HTTP actions via `spawn_action_consumer`, decode them, write to the PTY, then `ack_controller_action`.
  - Accept fast_path connections labeled `pb-terminal`, `pb-files`, etc., and treat every incoming `WireClientFrame::Input` as *human* keystrokes.
- Because the host ignores fast_path controller frames, the manager’s queue fills up, actions never reach the PTY, and Pong paddles never move. When HTTP poller is disabled (current rewrite-2 behavior), nothing ever drains the queue.

## Requirements
1. When a fast_path connection is labeled `pb-controller`, the host must:
   - Decode each frame as a controller action (same protobuf/message format as HTTP payloads).
   - Deliver the action bytes to the PTY (respecting sequencing and flow control identical to HTTP consumer).
   - Send the usual ACK (`HostActionAck`) back so Manager can dequeue.
2. Continue running the HTTP poller when the manager has not established a controller fast_path channel (fallback for legacy / air-gapped hosts).
3. Reuse existing logging namespaces (`controller.actions.*`). Add structured logs for fast_path consumption so operators can see when controller frames arrive/are acked.
4. Handle reconnects: when the `pb-controller` channel drops, resume HTTP polling until a new fast_path channel arrives.
5. Keep message ordering: only ack after the PTY write succeeds. Duplicate detection still happens via action ids.
6. No new env vars for typical users. Honor existing overrides (e.g., `PRIVATE_BEACH_CONTROLLER_MODE=http`, etc.) if they exist.

## Implementation Plan

### 1. Identify the relevant host code paths
- `apps/beach/src/server/terminal/host.rs`:
  - `Host::run` wires up fast_path connections via `FastPathClient::connect` and calls `spawn_input_listener` for each data channel.
  - `ActionConsumer` (search for `spawn_action_consumer`) handles HTTP polling/acking.
  - `HostFrame` / `WireClientFrame` definitions live in `crates/beach-proto/src/wire.rs`.
- `apps/beach-manager/src/state.rs` fast_path forwarder already labels controller connections as `pb-controller`. No manager changes needed.

### 2. Extend fast_path listener to detect controller channel
- When we build a `FastPathChannel` struct (look for `FastPathInbound`), capture the `client_label` string.
- In `spawn_input_listener`, branch on `client_label`:
  - If `== "pb-controller"`, do **not** feed into the host’s stdin queue. Instead, pass frames to a new async handler (`spawn_fast_path_controller_consumer`).
  - Otherwise keep the existing behavior.

### 3. Implement `spawn_fast_path_controller_consumer`
- Inputs: `Arc<HostContext>`, `FastPathChannel`, `controller_sink` (handle to PTY writer), `ack_client` (HTTP client or RPC handle for `/actions/ack`).
- Loop:
  1. Read `WireClientFrame` from the channel.
  2. Expect `Input { seq, data }` frames that encode `ControllerActionEnvelope` (protobuf struct used by HTTP). Use the same decode helper as HTTP consumer (`ControllerActionEnvelope::decode(&data[..])`).
  3. Deliver `action.payload` to `HostPtyWriter::write_controller_action` (mirrors HTTP path). This writes to the PTY and logs `controller.actions.apply`.
  4. After a successful write, call the existing `ack_controller_action(manager_client, session_id, action.id, ControllerAckTransport::FastPath)` to notify the manager. You can reuse the HTTP ack method if it accepts a transport enum; otherwise add one to keep metrics distinct.
  5. Send an explicit fast_path ack frame back to the manager (`HostFrame::InputAck { seq }`) so the WebRTC layer can release flow control.
- Handle errors: if decode fails, log `error` and keep the channel alive. If the channel closes, break so the host can fall back to HTTP polling.

### 4. Coordinate with existing HTTP consumer
- Add a small state machine inside `ActionConsumer` (or host context) that tracks `ControllerTransport`:
  - Start in `Unknown`.
  - When a fast_path `pb-controller` channel becomes healthy, switch to `FastPathPreferred` and pause the HTTP polling loop (but keep the task alive so it can resume quickly).
  - If the fast_path channel drops/errors, resume HTTP polling.
- Implementation sketch:
  - Wrap the HTTP polling future in `tokio::select!` that also listens for a `watch::Receiver<bool>` indicating “fast_path active”.
  - When `fast_path_active == true`, park the HTTP loop with `watch.changed().await` and skip polls; when it flips to false, continue polling immediately.

### 5. Logging & metrics
- Mirror existing log keys: `controller.actions.fast_path.start`, `controller.actions.fast_path.apply`, `controller.actions.fast_path.ack`.
- Include `session_id`, `action_id`, `seq`, and `transport=fast_path` fields so ops can grep.

### 6. Tests
1. **Unit tests** (Rust): add to `apps/beach/src/server/terminal/host.rs` or a new module:
   - Mock a `FastPathChannel` using a `tokio::sync::mpsc` pair and prove that when we feed an encoded `ControllerActionEnvelope`, the PTY writer receives bytes and ack function fires.
2. **Integration test** (optional but ideal): use the existing controller handshake fixture to spin up a fake manager, issue actions over fast_path, and ensure the host writes to PTY. If too heavy, document manual QA steps (see Verification below).

### 7. Verification / Manual QA
- Rebuild host + manager + beach road containers.
- Launch Pong (two public hosts + private agent) using `docs/helpful-commands/pong.txt` commands.
- Confirm host logs show `controller.actions.fast_path.apply` entries and no HTTP 404 spam.
- Observe paddles moving automatically.
- Double-check that stopping the fast_path channel (kill road) automatically falls back to HTTP.

### 8. Documentation & follow-ups
- Update `docs/helpful-commands/pong.txt` to note that controller actions now ride over fast_path by default.
- Add a troubleshooting note in `docs/private-beach/pong-controller-queue-incident.md` describing how to verify fast_path controller consumption.

## Prompt for Codex
Copy/paste the following to hand off implementation:

```
You are working in /Users/arellidow/development/beach. Implement the fast_path controller consumer described in docs/private-beach/fast-path-controller-consumer-plan.md:

1. In apps/beach/src/server/terminal/host.rs (and related modules), detect fast_path channels labeled pb-controller. Route their frames to a new async consumer that decodes ControllerActionEnvelope messages, writes the payload to the PTY (same path as HTTP actions), and issues controller ACKs plus fast_path InputAck frames. Keep existing fast_path behavior for other labels.
2. Introduce a transport-state switch so the HTTP action poller pauses while a healthy fast_path controller channel exists, and resumes if it drops. HTTP fallback must still work if no fast_path connection arrives.
3. Add structured logs/metrics (controller.actions.fast_path.*) aligned with existing controller logging patterns.
4. Write unit tests proving the fast_path consumer writes to the PTY and acks actions.
5. Update any relevant docs/helpful-commands to mention the new fast_path behavior.
6. Rebuild instructions: remind the user to rebuild docker images or binaries so the changes take effect.

After coding, summarize the changes and list verification steps (run Pong LHS/RHS hosts + agent).```

# Fast Path Parity Gap Plan

Owner: Codex (2025-11-12)

## Reality Check Summary
The initial plan in `docs/private-beach/fast-path-controller-consumer-plan.md` assumed the host already knows when a `pb-controller` channel connects and that fast_path frames carry `ControllerActionCommand` JSON. After implementing the plan we discovered three blockers:

1. **Missing client labels** – `transport::webrtc::connect_via_signaling` drops the `client_label` argument, so `spawn_input_listener` never receives `pb-controller` and the new fast_path path is unreachable.
2. **Wrong payload format** – Manager’s controller forwarder (`apps/beach-manager/src/state.rs::6394-6466`) serializes PTY bytes directly into `WireClientFrame::Input`. Our host code still tries to `serde_json::from_slice` each fast_path payload into `CtrlActionCommand`, so every action is rejected.
3. **HTTP-dependent ACKs** – `run_fast_path_controller_channel` requires `ManagerActionClient::ack_actions` to succeed before it sends `HostFrame::InputAck`. If the host is running in the new automatic mode (no manager URL/token), fast_path acks always fail and the queue never drains.

## Required Follow-up Work

### 1. Propagate controller labels through WebRTC negotiation
- Thread the optional `client_label` parameter from `negotiate_transport` → `transport::webrtc::connect_via_signaling` → `connect_answerer` → the `WebRtcConnection` metadata so every `SharedTransport` records the label.
- Add tests in `apps/beach/src/transport/webrtc/mod.rs` to ensure the assigned label is preserved.

### 2. Teach the host to consume fast_path PTY bytes
- Update `run_fast_path_controller_channel` (apps/beach/src/server/terminal/host.rs) to treat each `WireClientFrame::Input { data }` exactly like the HTTP consumer: the payload is already base64-decoded UTF-8 bytes. Remove the JSON decode step and feed the bytes straight into `PtyWriter`.
- Keep per-action logging (`controller.actions.fast_path.*`) so ops can confirm delivery.

### 3. Make fast_path ACKs self-contained
- Send `HostFrame::InputAck` immediately after a successful PTY write.
- Only enqueue HTTP `ack_actions` if a manager client exists; otherwise rely solely on the transport-level ack. Errors sending the optional HTTP ack must not cancel the fast_path ack.
- Record metrics/telemetry for both fast_path-only and fast_path+HTTP ack paths.

### 4. Resume/pausing logic audit
- Once items 1–3 land, re-test the `ControllerTransportSwitch` to ensure the HTTP poller pauses only when a real controller channel is alive and resumes if the channel drops.
- Add trace logs around transitions to catch regressions.

### 5. Tests & verification
- Unit test `run_fast_path_controller_channel` with synthetic `WireClientFrame::Input` payloads to prove PTY writes + InputAck happen without any manager client.
- Extend integration smoke test scripts (docs/helpful-commands/pong.txt) to mention the new requirement to rebuild the CLI after these changes.

## Acceptance Criteria
- Host logs show `controller.actions.fast_path.apply` / `.ack` entries as soon as `pb-controller` connects, even when no HTTP credentials are configured.
- Manager `controller.forwarder` logs report acks arriving (queue depth stays <10) and paddles move in the Pong demo without env vars.
- Killing Beach Road forces the host to resume HTTP polling within 1s.

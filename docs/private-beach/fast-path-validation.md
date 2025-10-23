# Fast-Path Validation Plan

Owner: Codex — 2025-06-19  
Scope: Harness ↔ Manager fast-path (WebRTC data channel) transport

## Automated Coverage (WIP)
- [ ] Add a harness integration test that injects a mock `FastPathSession` using in-process data channels:
  - Spin up a `FastPathClient` with deterministic SDP/ICE stubs and connect it to a mocked manager endpoint.
  - Publish synthetic `ActionCommand` envelopes over the manager side and assert the harness delivers them + returns `ActionAck` via the new fast-path code paths.
  - Verify Redis fallback is invoked when the data channel closes unexpectedly.
- [ ] Extend the manager test suite with a mocked harness peer:
  - Use the rust-webrtc testing utilities to create a pair of `RTCPeerConnection`s.
  - Drive `mgr-actions`/`mgr-acks`/`mgr-state` messages end-to-end and assert `AppState::ack_actions`/`record_state` are invoked (and metrics updated).
  - Gate the test behind `#[ignore]` until CI TURN/STUN fixtures are provisioned.

## Manual Fast-Path Smoke Test
1. **Environment**
   - Start Postgres + Redis (`docker compose up postgres redis`).
   - Set `BEACH_WEBRTC_DISABLE_STUN=1` on both the manager and the harness host to force TURN.
   - Optional: configure Beach Gate TURN output (e.g., `BEACH_GATE_TURN_URLS`, `BEACH_GATE_TURN_SECRET`, `BEACH_GATE_TURN_REALM`) if you need to point at a custom relay instead of the default lifeguard service.
2. **Manager**
   - `cargo run -p beach-manager` with the environment variables above.
   - Watch logs for `fast-path data channel ready` and `fast-path ack/state channel closed` lines to confirm loops latch correctly.
3. **Harness (CLI)**
   - Build the CLI: `cargo run -p beach -- run --private-beach <beach-id>`.
   - Confirm the harness registers, negotiates the WebRTC fast-path, and logs action receipt/ack events without hitting the HTTP endpoints.
4. **Verification**
   - Issue controller actions (e.g., `cargo run -p beach -- action send ...`) and ensure:
     - Manager logs show `fast-path data channel ready` and no HTTP poll/ack requests.
     - Prometheus metrics (`fastpath_actions_sent_total`, `fastpath_actions_fallback_total`, `fastpath_acks_received_total`, `fastpath_state_received_total`) reflect the traffic; `fastpath_channel_{closed,errors}_total` remain flat during stable runs.
   - Drop the data channel (e.g., kill the harness) and confirm manager logs report closure and the harness falls back to HTTP.
5. **Private Beach UI pairing workflow**
  - In the explorer sidebar, drag an application onto an agent (or use the assign menu on the application row). The assignment detail pane should slide in automatically so you can confirm the prompt/cadence before saving.
  - Agent tiles surface an “Assignments” bar that expands to show each controlled application with live transport/cadence badges; application tiles render controller badges in the header.
  - Keep the assignment pane open while triggering edits from another tab; fields and transport indicators should update in-place as SSE payloads arrive. Removing the assignment elsewhere should close the pane with a warning.
  - If the pane reports that controller pairing is not enabled, upgrade the manager to a build that includes the Track A pairing endpoints before proceeding with this validation.
  - `GET /sessions/:controller_id/controllers` should return the new pairing; tail `/sessions/:controller_id/controllers/stream` (SSE) to watch for live add/update/remove events while editing the relationship from the UI. The initial payload reports `transport_status.transport = "pending"`; once actions traverse the fast-path you should see an `"updated"` event with `"fast_path"` plus latency metadata. Force a fallback (e.g., drop the data channel) and confirm the stream emits another `"updated"` event with `"http_fallback"` and a populated `last_error`.
  - Tail the harness logs for `controller pairing update` and `controller lease renewed` lines to confirm the background runtime responds to SSE snapshots and keeps the controller lease fresh (look for fallback warnings if the stream disconnects). Updated logs surface `transport=<mode>`, `transport_latency`, and optional `transport_error` fields so you can validate telemetry without the UI.
  - Prometheus: verify `controller_pairings_active{controller_session_id="<controller>"}` increments and `controller_pairings_events_total{action="added"}` advances when the relationship is created; expect `controller_pairings_events_total{action="updated"}` to tick when the transport flips between fast-path and HTTP.
  - Remove an assignment from the explorer or detail pane to verify the DELETE flow, ensure agent bars collapse immediately, and confirm application badges clear without a manual refresh.

## TURN / STUN Configuration Knobs
- `BEACH_WEBRTC_DISABLE_STUN=1` — Set on **both** manager and harness/CLI to disable direct STUN lookups and force TURN. Use this during soak tests and whenever you need deterministic relay behaviour.
- `BEACH_GATE_TURN_URLS`, `BEACH_GATE_TURN_SECRET`, `BEACH_GATE_TURN_REALM`, `BEACH_GATE_TURN_TTL` — Gate-side knobs for issuing scoped TURN credentials; update these when validating against alternative relays.
- `BEACH_WEBRTC_LOG_LEVEL=debug` — Useful when diagnosing negotiation (surfaced by `webrtc` crate tracing).
- Remember to unset `BEACH_WEBRTC_DISABLE_STUN` once validation completes so normal ICE restarts re-enable direct P2P paths.

## Open Items
- Provision TURN credentials in CI so the mocked tests above can run automatically.
- Expand metrics: add `fastpath_actions_sent_total`, `fastpath_actions_recv_total`, `fastpath_state_recv_total`, and alert on data-channel closure rates once receive loops settle.

# Agent Command Gating Plan

## Problem
Controller commands currently queue even when:
1. The agent has no valid controller lease (never attached, lease expired, or token revoked).
2. The command targets a child session that is not the agent’s current attachment (wrong pairing, child offline, or a race during reassignment).
3. The child has not reached fast_path readiness (child session not fully attached, HTTP poller not yet active, or the `mgr-actions` channel is down).

This causes unbounded queue growth, confusing retries, and noisy controller logs. We need consistent, early rejection with actionable telemetry.

## Non-goals
- Do **not** change existing controller capabilities once the lease + attachment + fast_path checks pass.
- Do **not** touch docs/helpful-commands/pong.txt as part of this change.

## Requirements
- Drop (not queue) every controller command that fails lease or attachment validation.
- Return precise, machine-parseable error codes to callers so they can surface meaningful UX (e.g., “session not attached yet”).
- Emit structured logs and Prometheus counters for every drop reason.
- Respect fast_path readiness: only accept commands once the child session is attached **and** either the HTTP poller is actively draining commands or the `mgr-actions` channel is up.
- Backfill unit + integration coverage to lock the contract down.

## Design Overview

### Manager-side gating flow
1. **Entry points**: `queue_action`, `queue_actions_batch`, and the MCP `controller.queue` handler all funnel through a new `ControllerCommandGate` helper.
2. **Inputs collected** per request:
   - Controller lease (from token lookup) with `lease_id`, `controller_session_id`, `child_session_id`, `valid_until`.
   - Current manager session record for the child (attachment info, HTTP poller state, fast_path channel info).
   - Live controller pairing stored in `State::controller_sessions`.
3. **Validation helper** `ControllerCommandGate::validate(target_child_id)` returns `Result<(), GateError>` where `GateError` encodes the precise reason.
4. **Early drop path** when validation fails:
   - Do **not** enqueue or persist the action.
   - Return `409 CONFLICT` for mismatch conditions and `412 PRECONDITION_FAILED` for not-ready states (MCP equivalent errors mirror these codes via `error_code`).
   - Emit a structured log `controller.actions.drop` with fields `{reason, controller_session_id, child_session_id, lease_id, target_child_id}`.
   - Increment counter `controller_actions_dropped_total{reason="..."}` and histogram `controller_command_block_latency_seconds` measuring time since attach.
5. **Happy path** sets `reason="accepted"` gauge so we can alert if valid commands suddenly dry up.

### Gate validation matrix
| Check | Condition | Error code | Notes |
| --- | --- | --- | --- |
| Lease presence | Missing/expired lease | `missing_lease` | Drop before any state lookup.
| Lease target match | `lease.child_session_id != target_child_id` | `target_mismatch` | Covers stale controllers sending to previous children.
| Agent attachment | Manager session missing or child not attached | `child_not_attached` | Ensures child completed `attach_by_code`.
| Fast_path readiness | HTTP poller inactive **and** `mgr-actions` channel down | `fast_path_not_ready` | Accept when either transport is active.
| Agent-session pairing | Controller session not currently mapped to the lease | `session_not_bound` | Handles orphaned controllers.
| Child online | Child session flagged offline | `child_offline` | Drop to avoid queueing during downtime.

Each `GateError` implements `Display`, `error_code()`, and `metrics_reason()` so all callers behave consistently.

## Fast_path readiness tracking
- Extend `SessionRecord` with `fast_path_state: FastPathState` capturing `http_poller_active: bool`, `mgr_actions_channel: Option<ChannelId>`, and `last_ready_at` timestamp.
- Update:
  - HTTP poller startup/shutdown to toggle `http_poller_active`.
  - `mgr-actions` channel lifecycle hooks to set `mgr_actions_channel` and `fast_path_state.last_ready_at` when the child switches transports.
- `ControllerCommandGate` queries this state. Commands are accepted if **either** `http_poller_active` is true **or** `mgr_actions_channel.is_some()`.

## Telemetry
- **Logs**: `controller.actions.drop` (warn) + `controller.actions.accept` (debug) include `reason`, `error_code`, `lease_id`, `controller_session_id`, `child_session_id`, `target_child_id`, and `transport`.
- **Metrics**:
  - Counter `controller_actions_dropped_total{reason, transport}` increments on every rejection.
  - Gauge `controller_fast_path_ready{session}` set when a child first becomes eligible; used for alerting if readiness lags.
  - Histogram `controller_drop_age_seconds{reason}` measures `now - attach_completed_at` to see whether drops happen long after attach (indicates stale agents).

## Test Coverage
1. **Unit tests (Rust)** in `apps/beach-manager/src/state.rs` (or a new `controller_gate.rs`).
   - `missing_lease` → returns error, no queue writes.
   - `target_mismatch` → verifies log context + metrics reason.
   - `child_not_attached` → ensures we still log the attempted child id.
   - `fast_path_not_ready` → verifies we require either HTTP poller active or mgr-actions up.
   - `session_not_bound` / `child_offline` scenarios.
   - Assert Prometheus counters increment with the right `reason` label and error payload matches spec.
2. **Integration tests** under `tests/manager_controller.rs` (or add new file):
   - **Pre-attach rejection**: spin up agent, enqueue before attach, expect HTTP 412 + `fast_path_not_ready`, queue remains empty, counter increments.
   - **Lease mismatch rejection**: attach two agents, send command from agent A to agent B’s child, expect HTTP 409 + `target_mismatch`, verify log contains both session ids.
   - **Happy path**: attach + enable fast_path, send command, ensure queue length increments and no drop counter change.
   - **Fast_path transition**: start with HTTP poller active (accept), then disable poller without mgr-actions; subsequent command should be rejected until mgr-actions comes up.
3. **Metrics smoke test** (optional integration): run command gating through to verify `/metrics` exposes the new counter and gauge labels.

## Rollout
1. Ship behind env flag `CONTROLLER_STRICT_GATING` (default `false`).
2. Enable in staging with high log verbosity; confirm counters move as expected.
3. Flip to `true` in production after validating no spike in `target_mismatch` for >24h.
4. Keep legacy behavior behind the flag for one release as a safety hatch.

## Open Questions
- Should we throttle clients that repeatedly fail validation (e.g., `missing_lease` 100x/min)? A follow-up can bucket and rate-limit.
- Do we want to surface `fast_path_not_ready` in the UI immediately, or keep it internal to the agent CLI for now?

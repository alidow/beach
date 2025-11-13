# Pong Agent Hyperactivity Mitigation

## Goal
Ensure the demo Pong agent behaves like a real automation client:
- It only issues controller commands after it has verified that both child sessions (LHS/RHS players) are attached and reachable.
- It backs off when Manager replies with throttling or “not attached” errors.
- It stops sending once queues report saturation.

## Behavior Changes
1. **Connection awareness**: the agent must wait for confirmation that:
   - Manager granted a controller lease (`attach_by_code` complete, lease active).
   - Fast-path channel (or HTTP poller) exists to each child.
2. **Rate limiting / pacing**:
   - Introduce a per-child command budget (e.g., max 30 commands/sec).
   - Use exponential backoff when Manager responds with 429/409 or `not_attached`.
3. **Lifecycle hooks**:
   - Pause gameplay when a child disconnects; resume only after reattach.
   - Log “waiting for players” state so UX explains the pause.

## Implementation Steps
1. **State tracking inside `run-agent.sh` / Python agent**:
   - Subscribe to Manager events (or poll) to learn when each player is “ready”.
   - Maintain a finite-state machine: `AwaitingPlayers → Ready → Running → Paused`.
2. **Command scheduler**:
   - Wrap existing game loop in a scheduler that:
     - Checks `player_ready` before enqueuing.
     - Yields sleep/backoff durations when manager returns non-200 responses.
     - Stops sending once `queue_depth` metrics exceed a threshold (read via Manager API).
3. **Logging / Telemetry**:
   - Emit logs when the agent starts/stops sending commands and when it encounters throttling.
   - Optionally expose a `/status` HTTP endpoint (or stdout heartbeat) so tests can assert readiness.
4. **Tests**:
   - Unit test the scheduler: given mock readiness events, confirm it suppresses sends until both players ready.
   - Integration smoke test: bring up two dummy hosts, deliberately delay `attach_by_code`, ensure agent waits.
5. **Rollout**:
   - Feature flag to enable pacing (e.g., `PONG_AGENT_SAFE_MODE=1`) so we can toggle during testing.

## Acceptance Criteria
- Agent never logs `queue_action HTTP 429` once players are attached.
- Agent emits “waiting for players” while attach is pending.
- During network interruptions the agent pauses automatically and resumes without saturating queues.

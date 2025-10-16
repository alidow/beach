# Intra-Private-Beach Orchestration

## Goals
- Allow any session that belongs to a Private Beach to observe and coordinate other sessions through MCP APIs exposed by the Private Beach manager.
- Maintain a low-latency, server-hosted cache of session state (terminal buffers, GUI frames, metadata) to enable instant cross-session visibility.
- Provide an action dispatch layer for keyboard, mouse, and byte-sequence injections so agents and humans can steer remote sessions programmatically.
- Showcase the capability with a flagship Pong demo that mixes TUI, Windows GUI (via Beach Cabana), and an MCP-driven “manager” agent.

## Capabilities Overview
- **Session Directory:** MCP tool returning metadata for active sessions (type, location, capabilities, status heartbeat).
- **State Snapshot:** MCP tool streaming or snapshotting current render state from the replicated cache; supports terminal screen diff or GUI frame reference.
- **Action Dispatch:** MCP command queue routed over a fast message bus (Redis streams or NATS); targets include terminal byte writes and GUI input events.
- **Access Control:** Private Beach manager enforces that only sessions within the same private beach (or explicitly shared contexts) can query/act on each other.
- **Observability Hooks:** Every action and state request is auditable for future billing, debugging, and compliance tooling.

## Reference Architecture
1. **Session Agent SDK**
   - Sessions establish a backchannel to the manager upon join (MCP handshake).
   - Sessions publish capabilities (e.g., `supports_terminal_bytes`, `supports_gui_pointer`).
2. **State Replicator**
   - Receives incremental updates from each session (terminal diff, GUI frame hash, cursor state).
   - Normalizes and stores latest view in Redis (per-session cache key).
   - Supports change feeds so watchers can subscribe instead of polling.
3. **Command Bus**
   - Commands emitted by manager or other sessions are appended to a low-latency queue.
   - Target session consumes commands via persistent MCP stream; applies, acknowledges, and logs.
4. **Policy & Mediation Layer**
   - Validates permissions, rate limits, and conflict resolution (e.g., multiple controllers).
   - Exposes admin override for human operator to pause automation or force control handoff.

## MCP Surface (Draft)
- `list_sessions`: Returns IDs, labels, media type (`terminal`, `cabana_gui`, `scoresheet`), current controller, and health metrics.
- `get_session_state`:
  - `mode`: `snapshot` (returns last known render) or `stream` (opens streaming diff channel).
  - `format`: `terminal_ansi`, `structured_grid`, `gui_frame_ref`.
- `queue_action`:
  - Supports `terminal_write`, `key_event`, `pointer_move`, `pointer_click`, `pointer_scroll`.
  - Options for priority, deduplication token, and expiration.
- `set_ball_state`/`custom_actions`: Placeholder for game-specific or domain-specific verbs surfaced by client sessions.

## Pong Showcase Flow
1. **Left Paddle (TUI)**
   - Terminal app exposes `supports_ball_state`, `supports_terminal_bytes`.
   - Manager pushes paddle moves via `terminal_write` commands.
2. **Right Paddle (Windows GUI)**
   - Beach Cabana surfaces pointer/keyboard control to the manager.
   - Manager obtains GUI raster snapshot via `get_session_state`.
3. **Manager Agent**
   - Polls both sides using high-frequency `get_session_state` in streaming mode.
   - Calculates ball trajectory, emits paddle commands, and sets cross-session ball state when ownership changes.
4. **Scoreboard Session**
   - Simple TUI that updates via `terminal_write` triggered by the manager when a side scores.
5. **Spectator View**
   - Private Beach dashboard arranges all four sessions; observers can watch in real time without interfering.

## Open Questions
- How do we handle conflicting control when a human attempts to take over a session already driven by an agent?
- Should the state cache store raw frames or normalized vector representations to optimize network cost?
- What guarantees do we offer around action ordering, especially when multiple sessions queue actions against the same target?
- Do we need per-action confirmation hooks (ack/nack) for audit logs and UI display?
- How frequently can we poll/stream state before hitting performance ceilings on terminals and Cabana streams?
- What sandboxing is required so that an agent cannot exfiltrate or misuse another session’s credentials or file system?

## Next Steps
1. Design MCP schema messages for the three core tools and define authentication tokens per session.
2. Prototype the state replicator path using existing terminal diff events from `apps/beach`.
3. Define the command bus interface and extend terminal/GUI clients with a lightweight consumer loop.
4. Build the Pong demo pipeline as an integration test harness to validate latency and conflict control scenarios.
5. Iterate on dashboards to visualize agent control, queue backlog, and session health indicators.


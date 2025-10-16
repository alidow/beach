# Private Beach Pong Showcase

## Purpose
- Demonstrate cross-session orchestration, harness-driven instrumentation, and agent mediation in a playful scenario that maps directly to real collaboration use cases.
- Stress-test the Private Beach MCP surfaces (state snapshots, action dispatch, optional shared storage) under fast event cycles.
- Provide a demo that is visually compelling in the dashboard layout while remaining technically feasible across terminal and GUI clients.

## Experience Overview
1. **Landing View**
   - Private Beach dashboard opens to a preset layout showing four tiles: left paddle TUI, right paddle GUI (Beach Cabana), scorecard TUI, and the manager agent console (collapsed by default).
   - A hero banner or overlay briefly explains the roles and gives users a “Start Match” button.
2. **Match Start**
   - When triggered, the manager deploys initial state (ball spawn, velocity) via MCP to both paddles and displays a countdown overlay visible in both tiles.
   - Scoreboard initializes to 0–0 with animated confirmation.
3. **Gameplay Loop**
   - Users observe paddles responding in real time to ball motion; optional toggle lets humans take over control to highlight cooperative handoff.
   - Manager console (if expanded) shows stream of MCP calls, acknowledgements, and real-time stats (latency, FPS, prediction confidence).
4. **Goal Events**
   - When a side scores, scoreboard tile updates with celebratory animation; ball resets with velocity adjusted for difficulty.
   - Dashboard shows toast notifications and increments a match timer.
5. **Match End**
   - After predefined duration or points limit, manager announces winner, posts a highlight summary, and offers restart or deep-dive insights (action log download, agent analysis).

## Session Roles
- **Left Paddle (TUI)**
  - Runs inside `apps/beach` terminal client with no Private Beach awareness.
  - Accepts paddle commands as byte sequences (`W`/`S` or arrow equivalents) and ball state updates via MCP.
  - Harness captures terminal diffs and derives paddle/ball cues for the manager.
- **Right Paddle (GUI)**
  - Windows application streamed via Beach Cabana with zero knowledge of Beach orchestration.
  - Receives pointer and keyboard events; displays rich visualization to emphasize GUI control.
  - Cabana harness publishes compacted frame metadata (paddle position, contact events, rendering FPS).
- **Scoreboard (TUI)**
  - Minimal terminal app showing current score, rally count, and match time.
  - Accepts atomic updates from manager to avoid desync; harness mirrors state for spectators.
- **Manager Agent**
  - Fast open-source LLM (or deterministic Rust controller) operating in a terminal session.
  - Consumes streaming state snapshots, computes trajectories, and queues control actions.
  - Maintains authoritative match state in-memory (optionally mirrored to shared storage for telemetry/replay).

## Functional Requirements
- **State Synchronization**
  - Harnesses stream render diffs to the Private Beach cache at sub-100ms intervals.
  - Manager subscribes to both streams and derives ball position; paddles remain unaware of Private Beach mechanics.
- **Action Dispatch**
  - Manager issues prioritized commands with acknowledgement; target harnesses must process within 50ms budget.
  - Conflict resolution policy grants the manager “primary” control, with manual override button in UI.
  - Optional direct WebRTC data channels eliminate message-bus latency; harness buffers commands locally when offline.
- **Shared Storage (Optional)**
  - Not required for core gameplay; manager can write match summaries post-game if we want persistence or replay.
- **Observability**
  - Unified log stream records action history, latency measurements, and errors.
  - Metrics feed surfaces in dashboard (ticks per second, command queue depth).

## UX Principles
- **Clarity:** Each tile displays a subtle overlay describing its role and current controller.
- **Responsiveness:** Visual feedback (glow, pulse) accompanies every scored point or control handoff.
- **Transparency:** Manager console reveals MCP traffic to build trust in automation; spectators can peek without technical clutter.
- **Agency:** Toggle controls allow human takeover; UI warns when automation is paused or resumed.
- **Shareability:** One-click “Share Highlight” exports a short clip or animated GIF composed from cached frames.

## Interaction States
- **Pre-Game:** Instructions panel explains controls, feature callouts, and invites users to start the match.
- **Live Play:** Tiles show real-time action; top bar tracks score/time; notifications area reports events.
- **Manual Control:** When a user clicks “Take Control,” UI locks out the agent, shows countdown, and transitions control.
- **Recovery:** If a session drops, overlay displays reconnect status; manager pauses game and posts status updates.
- **Post-Game:** Summary modal displays key metrics (rallies, longest volley, agent reaction time) with restart CTA.

## Technical Flow Snapshot
1. Private Beach manager creates session graph and registers each client with capabilities.
2. State replicator ingests frame/terminal diffs and stores normalized snapshots.
3. Manager agent subscribes to streams, calculates moves, and emits commands via brokered queue or negotiated WebRTC channel.
4. Harnesses consume commands, apply inputs, and acknowledge completion; acknowledgements loop back to manager UI and latency tracker.
5. Manager retains authoritative state; scoreboard renders whatever the manager writes, and optional persistence hooks run after the match.

## Demo Instrumentation
- Latency tracer measuring mcp-request → action-applied timings.
- Frame consistency checker comparing predicted vs. actual ball position.
- Optional replay recorder using cached state to produce time-lapse after match.
- Telemetry dashboard panel embedded in manager console for debugging.

## Risks & Mitigations
- **Input Lag:** Optimize command queueing, prefetch paddle position, and allow speculative moves.
- **State Drift:** Keep physics exclusively in the manager; harnesses just render manager-issued ball snapshots to avoid divergence.
- **Viewer Overload:** Provide minimal/advanced display modes to avoid overwhelming new users.
- **Agent Failure:** Include fallback script that takes over if primary agent session crashes.
- **Security:** Enforce scoped tokens so demo sessions cannot access unrelated private beach data.

## Implementation Checklist (Draft)
1. Build minimal paddle TUI and GUI clients with deterministic physics hooks.
2. Implement state snapshot streaming & cache validation probes.
3. Extend MCP surface with action queue semantics and acknowledgements.
4. Craft dashboard layout (responsive grid, overlays, control toggles).
5. Develop manager agent logic and logging instrumentation.
6. Add replay/log export utilities and highlight builder.

## Open Questions
- Should the ball physics live exclusively in the manager or be co-simulated by paddles for redundancy?
- What is the ideal cadence for state updates to balance fidelity vs. bandwidth?
- How do we visually indicate when spectators vs. controllers are interacting?
- Do we require user accounts/auth to “take control,” or can share links grant temporary control tokens?
- Can we generalize the overlay/interaction framework for future demos beyond Pong?

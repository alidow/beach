# Pong Showcase – Remaining Work (Trace UX & Automation)

This note breaks down the unfinished portions of the original wiring plan so another engineer/agent can pick them up immediately. Each section captures the goal, current gaps, and the concrete tasks/code areas to touch.

---

## 1. Global Trace Observability

**Goal**  
When a user flips “Trace communications” on an agent tile, every surface (dashboard UI, manager logs, agent logs) should expose the relevant trace id with an easy toggle to view only trace-aware events. The original plan’s success criteria included “coherent logs across dashboard, manager, and agent.”

**Current State / Gaps**
- Agent tiles store `traceEnabled` + `traceId`, and FlowCanvas batches controller assignments with `X-Trace-Id`. However:
  - No dashboard UI aggregates trace-enabled edges or emits trace-tagged console logs beyond API calls.
  - Manager logs the trace id only for controller assignments and queue_action requests; SSE emissions (`controller_pairing` + `state` streams) and MCP hint responses ignore it.
  - There is no “Trace Viewer” overlay in the dashboard to inspect trace activity (who was paired, when failures happened, etc.).
  - The agent harness logs trace ids for MCP actions but does not expose a filtered view or separate sink for a given trace id.

**Implementation Tasks**
1. **Dashboard trace overlay**
   - Add a “Trace Monitor” button to `AssignmentEdge` when trace is enabled. Clicking opens a panel in `FlowCanvas` showing:
     - Trace id, onboarding prompt summary.
     - Most recent pairing sync attempt timestamps (success/failure) pulled from local state.
     - A live log stream (maybe via `console.info` interception or a dedicated `useTraceLog` hook) filtering client-side events with that trace id.
   - File(s): `apps/private-beach-rewrite-2/src/features/canvas/FlowCanvas.tsx`, `AssignmentEdge.tsx`, new overlay component.

2. **Dashboard logging**
   - Update places where we `console.info` connection events (`useSessionConnection`, `viewerConnectionService`, etc.) to include `trace_id` in JSON payloads when the tile’s agent trace is enabled.

3. **Manager logging**
   - Thread trace id into additional endpoints:
     - SSE `stream_state` and `stream_controller_pairings`: accept `X-Trace-Id` and log along with emitted events.
     - `onboard_agent` and `list_sessions` responses: include trace metadata in logs when the agent session has tracing on.
   - Files: `apps/beach-manager/src/routes/{sse.rs,sessions.rs}`, `state.rs`.

4. **Trace toggle audit**
   - Ensure toggling trace off removes all headers/state and clears cached errors so the overlay disappears.

---

## 2. Prompt Pack Consumption (Agent Harness)

**Goal**  
The demo agent should treat the manager’s prompt pack as the source of truth for roles/responsibilities and available MCP bridges. That means:
- Auto-configuring the labels shown in the TUI.
- Hinting which MCP bridge endpoints exist (e.g., `private_beach.subscribe_state`) and whether they are connected.
- Optionally altering autopilot behavior based on instructions (e.g., “serve ball to lhs first”).

**Current Gaps**
- We now fetch the prompt pack when autopairing, but we only log its instructions line-by-line; no behavioral changes happen.
- Session roles (`lhs`, `rhs`, etc.) still come only from CLI args/metadata, not from prompt content.
- There is no UI indicator of which MCP bridges are available.

**Implementation Tasks**
1. Extend `AgentApp` to parse `prompt_pack`:
   - Show a dedicated “Prompt” panel (top of the TUI) with the role/responsibility summary.
   - Persist the prompt pack contents so the operator can re-open them (e.g., pressing `P` toggles the prompt view).

2. Use prompt directives:
   - If instructions mention a preferred opening move (e.g., targeted serve), feed that into `_maybe_spawn_ball` or `_drive_paddle`.
   - At minimum, expose hooks/placeholders to interpret structured instructions (`options` field).

3. MCP bridge awareness:
   - Display the list of `mcp_bridges` returned by onboarding (name + endpoint + status). Highlight whether we’ve opened a state subscription for each child.
   - Later, these entries could inform future enhancements (e.g., automatically opening additional bridges).

Files to modify: `apps/private-beach/demo/pong/agent/main.py` (`AgentApp`, `_draw`, key handling), potential new `PromptPanel` helper.

---

## 3. Automated Pong Showcase Validation

**Goal**  
Have a repeatable script/test that exercises the full flow:
1. Launch mock LHS/RHS player TUIs.
2. Launch the agent TUI (or a headless harness variant).
3. Use the dashboard API (or CLI script) to attach sessions, save agent role/responsibility with trace enabled, draw edges, and ensure `batchControllerAssignments` succeeds.
4. Verify that:
   - Trace logs appear (manager + agent).
   - Controller pairing SSE stream reports child attachments.
   - Polling fallback engages when we simulate an SSE drop.

**Implementation Tasks**
1. **Test harness script** (e.g., `scripts/pong_showcase_smoke.ts` or `.py`):
   - Spawns the three TUIs (possibly via tmux panes or subprocesses).
   - Uses the dashboard API client to call `attach_by_code`, `put_canvas_layout`, and `batchControllerAssignments`.
   - Monitors manager logs (or SSE) to ensure the trace id flows through.

2. **CI-friendly mode for the agent**:
   - Add a headless flag (e.g., `--no-ui`) that runs the agent logic without curses so the script can assert behavior programmatically.

3. **Documentation** (`docs/agent-wiring/pong-showcase-wiring.md`):
   - Add a “Validation” section detailing how to run the smoke test and interpret pass/fail criteria.

Deliverable: a script or test target that can be invoked locally/CI to give a green/red result for the entire Pong showcase wiring.

---

## Summary
The codebase now supports trace-aware assignments and prompt ingestion, but to hit the original success criteria we still need:
- A cohesive trace debugging experience (UI + backend logging).
- Deep prompt-pack integration in the agent harness UI/behavior.
- An automated smoke test that proves the Pong showcase works end-to-end.

The tasks above isolate the required changes by file so another contributor can pick them up quickly.

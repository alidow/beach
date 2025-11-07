# Private Beach Pong Showcase — Agent Wiring Plan

Purpose: define the concrete wiring needed so the Pong showcase behaves exactly as described: an Agent tile manages two Application tiles (players), edges declare control relationships, the agent receives initial prompts and child state, and communications are optionally traceable.

This plan is written to be implementable immediately by another engineer/agent. It references actual code paths and APIs in this repo.

## Success Criteria
- Agent tile can be attached to a live agent session (same attach UX as Application tiles).
- Saving Agent role/responsibility generates an initial prompt for the agent via the manager’s onboard endpoint, and stores it in session metadata for retrieval.
- Creating/saving an edge (Agent → Application) creates/updates a controller pairing in the manager with instructions mapped to `prompt_template` and cadence mapped from the edge’s update mode.
- The agent process automatically discovers its children from controller pairing events, fetches an initial PTY snapshot per child, subscribes to live state, and respects the configured cadence (SSE, push, or polling fallback).
- Optional “trace” switch yields coherent logs across dashboard, manager, and agent for agent↔child interactions.

## Current State (as-coded)
- Tiles: Agent and Application tiles exist; edges (AssignmentEdge) collect `instructions`, `updateMode` (`idle-summary` | `push` | `poll`), and `pollFrequency`.
  - UI: `apps/private-beach-rewrite-2/src/features/canvas/FlowCanvas.tsx`
  - Data: `apps/private-beach-rewrite-2/src/features/tiles/types.ts`
- Persistence: Layout + relationships serialize to manager CanvasLayout metadata (`agentRelationships`).
  - `apps/private-beach-rewrite-2/src/features/tiles/persistence.ts`
  - Manager stores/retrieves layout only.
- Session Attach: Both Application and Agent tiles embed `ApplicationTile`, so both can attach via session id + passcode and render live terminal.
  - `apps/private-beach-rewrite-2/src/features/tiles/components/TileFlowNode.tsx`
- Gaps:
  - Edge actions don’t call manager pairing APIs; no runtime effect.
  - No agent onboarding or prompt delivery path wired from Agent tile save.
  - No notification payload when edges change; agent has to self‑discover.

## Target End‑to‑End Flow
1) User attaches LHS/RHS player sessions and a single agent session (all via tile attach UI).
2) User defines Agent role/responsibility and saves → dashboard calls manager `onboard_agent` and persists the prompt in agent session metadata.
3) User draws edges from agent → players and saves edge settings (instructions + update mode) → dashboard calls manager `batchControllerAssignments` to create/update controller pairings.
4) The agent TUI auto‑listens to its “controller pairings” event stream and, on each Added/Updated event, fetches the child’s latest terminal snapshot, starts a live SSE stream, and chooses cadence based on the pairing/update‑mode mapping. It then controls paddles accordingly and logs actions.
5) Optional: enabling “trace” yields coherent, correlated logs across UI, manager, and agent.

## API Surfaces To Use
- Dashboard (rewrite‑2) → Manager
  - `PUT /private-beaches/:id/layout` (existing): store relationships as metadata (already done).
  - `POST /sessions/:controller_id/controllers` or batch: `POST /private-beaches/:id/controller-assignments/batch` — create/update controller pairings.
  - `POST /agents/onboard` — get `prompt_pack` + MCP bridge hints for the agent session.
  - `PATCH /sessions/:id` — update session metadata (store prompt pack, role=agent, etc.).
- Agent → Manager
  - `GET /sessions/:controller_id/controllers/stream` — SSE: pairing add/update/remove events.
  - `GET /sessions/:child_id/state` — latest terminal snapshot (bootstrap before SSE).
  - `GET /sessions/:child_id/state/stream` — live state SSE (already used for children in the demo).
  - `POST /sessions/:session_id/controller/lease` — renew agent’s lease (already in demo agent).
  - `POST /sessions/:child_id/actions` — queue `terminal_write` commands (already in demo agent).

## Data Mapping
- Edge → Manager Pairing
  - Edge fields: `instructions`, `updateMode`, `pollFrequency`.
  - Manager pairing fields: `prompt_template`, `update_cadence` (`fast|balanced|slow`).
  - Proposed mapping:
    - `idle-summary` → `slow`
    - `push` → `fast`
    - `poll` → `balanced` (edge `pollFrequency` retained in layout metadata; the agent uses it if it needs to poll).

- Agent Role/Responsibility → Initial Prompt
  - Compose a prompt template that includes:
    - Agent role and responsibilities (from Agent tile).
    - Operating instructions for the harness: how to identify children, how to send commands (by session id), cadence expectations, and how to log.
  - Store in session metadata and/or pass as `prompt_template` when creating pairings for additional context.

## Dashboard (rewrite‑2) Work
1) Agent Tile Save → Onboard + Metadata
  - Trigger when an Agent tile with an attached `sessionMeta.sessionId` is saved.
  - Call `onboard_agent(sessionId, template_id='pong', scoped_roles=['agent'], options={})`.
  - Merge the returned `prompt_pack` into the agent session’s metadata via `updateSessionMetadata` (and ensure metadata.role = 'agent').
  - Persist Agent tile role/responsibility in CanvasLayout (already handled).

2) Edge Save → Manager Pairing
  - On `RelationshipEdge` Save in `FlowCanvas`:
    - Resolve source agent session id and target child session id from tile state (`tile.sessionMeta.sessionId`). If either is missing, show UI hint and skip pairing.
    - Build `prompt_template` by combining Agent role/responsibility with Edge instructions.
    - Map `updateMode` to `update_cadence` (see mapping above).
    - Call `batchControllerAssignments(privateBeachId, [{ controller_session_id, child_session_id, prompt_template, update_cadence }])`.
    - Optionally persist `pollFrequency` alongside relationship in layout (already serialized) for agent consumption.

3) Trace Flag (Optional)
  - Add a “Trace communications” toggle per Agent tile and store in layout metadata (and/or a global toggle in the page). Thread it into API calls as `metadata.trace=true` on the agent session and include `trace_id` in any action payloads emitted by the dashboard in future.

## Agent (demo TUI) Changes
1) Pairing Event Subscription
  - Add a second SSE consumer to the agent:
    - `GET /sessions/:controller_id/controllers/stream`
    - Handle events `{ action: 'added' | 'updated' | 'removed', pairing: { child_session_id, prompt_template, update_cadence } }`.
    - On Added/Updated:
      - Fetch `GET /sessions/:child_id/state` once to obtain the latest `StateDiff` for bootstrap.
      - Begin `GET /sessions/:child_id/state/stream` for live updates.
      - Cache cadence choice (SSE default; if update mode is `poll`, schedule `GET /state` every N seconds as a fallback if SSE is down).
    - On Removed: stop streaming/polling that child.

2) Prompt/Instructions Consumption
  - Read `prompt_template` from pairing events and/or the agent session’s metadata `prompt_pack` (via `GET /private-beaches/:id/sessions` and filtering by `session_id`).
  - Display in the agent TUI header/log; use it to set initial behavior.

3) Polling Strategy
  - Default to SSE (fast path if available) for child state.
  - If the edge’s `updateMode` was `poll`, respect the saved `pollFrequency` if SSE is unavailable or explicitly disabled. Schedule periodic `GET /sessions/:child_id/state` and treat it as a diff source.

4) Tracing (Optional but Recommended)
  - When `trace` is enabled via the agent session’s metadata, include a `trace_id` on logs and mirror the manager’s logs with the same id. Log a compact JSON line for every control and state event.

## Security & Auth
- Dashboard needs a token with scopes:
  - `pb:beaches.read`, `pb:beaches.write` (layout)
  - `pb:sessions.read` (credentials, snapshots)
  - `pb:control.write` (create pairings)
- Agent needs `pb:sessions.read` (SSE state) and `pb:control.write` to queue actions.
- No controller lease is required for the dashboard to create pairings; the agent itself must hold/renew its own controller lease to send actions to children (already implemented in the demo agent).

## Minimal Implementation Steps (Checklist)
Dashboard (rewrite‑2):
1) In `FlowCanvas.tsx`, on `handleEdgeSave`, resolve `source`/`target` session ids from tile state and call `batchControllerAssignments` with mapped cadence and a `prompt_template` composed from Agent tile + edge instruction. On success, close the edge editor; on failure, surface an inline error.
2) In `TileFlowNode.tsx`, when saving an Agent tile that has `sessionMeta.sessionId`, call `onboard_agent` and persist the returned `prompt_pack` to the agent session via `updateSessionMetadata` (metadata.role='agent'). Keep the CanvasLayout agent metadata unchanged.
3) Add an optional “Trace” toggle and thread `trace` via session metadata and API calls (optional).

Agent (demo):
4) Add `subscribe_controller_pairings(controller_session_id)` SSE consumer; on `Added`/`Updated`, fetch `GET /sessions/:child_id/state` (bootstrap) then start `state/stream`. Honor cadence mapping as above. On `Removed`, detach.
5) Read the agent’s own session metadata to obtain `prompt_pack` and trace config; display prompt in the TUI and log trace id if present.

Manager (optional niceties):
6) No API changes required. Consider extending `ControllerPairing` SSE events to include `poll_frequency` mirrored from Canvas metadata in the future. Today, the agent can read it from layout metadata if you expose it via a helper.

## Action/Notification Schema (for reference)
- Pairing event (manager → agent SSE):
```json
{
  "controller_session_id": "sess-agent",
  "child_session_id": "sess-lhs",
  "action": "added",
  "pairing": {
    "pairing_id": "…",
    "prompt_template": "… (from edge + agent role) …",
    "update_cadence": "fast|balanced|slow",
    "transport_status": { "transport": "pending|fast_path|http_fallback" },
    "created_at_ms": 0,
    "updated_at_ms": 0
  }
}
```

- Initial snapshot (dashboard/agent fetch): `GET /sessions/:child_id/state` returns `StateDiff` (already defined in the repo).

## Testing Plan
1) Launch two player TUIs and one agent TUI (see `apps/private-beach/demo/pong/README.md`). Attach sessions in dashboard tiles.
2) Save Agent role/responsibility; verify `onboard_agent` call and metadata update (observe network calls and session metadata).
3) Draw edges and save; verify `batchControllerAssignments` calls and ControllerPairing entries (and SSE events visible to the agent).
4) Confirm the agent starts receiving events, fetches snapshots, streams state for both children, and begins controlling paddles.
5) Toggle update modes; verify cadence behavior (SSE by default; induce SSE failure to see polling fallback if configured).
6) Enable trace; confirm correlated logs in dashboard console, agent logs, and manager logs with a shared trace id.

## Notes & References
- Dashboard attach + viewer: `apps/private-beach-rewrite-2/src/components/ApplicationTile.tsx`
- Edge UI & store: `apps/private-beach-rewrite-2/src/features/canvas/FlowCanvas.tsx`, `apps/private-beach-rewrite-2/src/features/tiles/store.tsx`
- Layout persistence: `apps/private-beach-rewrite-2/src/features/canvas/useTileLayoutPersistence.ts`
- Manager pairing APIs: `apps/beach-manager/src/routes/sessions.rs` and `apps/private-beach/src/lib/api.ts` (`createControllerPairing`, `batchControllerAssignments`)
- Agent demo: `apps/private-beach/demo/pong/agent/main.py`


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
    - `poll` → `slow` (keep explicit `pollFrequency` for the agent to enforce; cadence remains conservative and does not imply push).

- Agent Role/Responsibility → Initial Prompt
  - Compose a prompt template that includes:
    - Agent role and responsibilities (from Agent tile).
    - Operating instructions for the harness: how to identify children, how to send commands (by session id), cadence expectations, and how to log.
  - Store in session metadata and/or pass as `prompt_template` when creating pairings for additional context.

## Metadata Schema (explicit)
- Agent session metadata (stored via `updateSessionMetadata`):
```json
{
  "role": "agent",
  "agent": {
    "profile": "pong",
    "prompt_pack": { /* opaque onboard_agent payload */ },
    "trace": { "enabled": true, "trace_id": "pb-trace-<uuid>" }
  }
}
```
- CanvasLayout metadata (persisted by dashboard):
```json
{
  "agentRelationships": {
    "<relId>": {
      "id": "<relId>",
      "sourceId": "<agentTileId>",
      "targetId": "<appTileId>",
      "sourceSessionId": "sess-agent",
      "targetSessionId": "sess-player",
      "instructions": "control LHS…",
      "updateMode": "idle-summary|push|poll",
      "pollFrequency": 1
    }
  },
  "agentRelationshipOrder": ["<relId>"],
  "createdAt": 0,
  "updatedAt": 0
}
```

If manager reflection of `pollFrequency` into pairing events is added later, prefer that; otherwise the agent will fetch layout to read this field (matching on `targetSessionId === child_session_id` and `sourceSessionId === controller_session_id` to recover the relationship id).

## Dashboard (rewrite‑2) Work
1) Agent Tile Save → Onboard + Metadata
  - Trigger when an Agent tile with an attached `sessionMeta.sessionId` is saved.
  - After attach: override the default role set by `ApplicationTile`.
    - Today `ApplicationTile` updates role to `'application'` post-attach. For Agent tiles, immediately follow with `updateSessionRoleById(sessionId, 'agent', …)` to flip the role before onboarding.
  - Call `onboard_agent(sessionId, template_id='pong', scoped_roles=['agent'], options={})`.
  - Merge the returned `prompt_pack` into the agent session metadata at `metadata.agent.prompt_pack` and set `metadata.role = 'agent'`. When patching metadata, re-use `buildSessionMetadataWithTile` (or equivalent) so existing `sessionMeta` / viewer state blobs stay intact.
  - Failure handling: if `onboard_agent` fails, persist tile role/responsibility locally and surface a non-blocking warning; add a retry button. Do not leave session metadata half-updated.
  - Persist Agent tile role/responsibility in CanvasLayout (already handled).

2) Edge Save → Manager Pairing
  - On `RelationshipEdge` Save in `FlowCanvas`:
    - Resolve source agent session id and target child session id from tile state (`tile.sessionMeta.sessionId`). If either is missing, show UI hint and skip pairing.
    - Build `prompt_template` by combining Agent role/responsibility with Edge instructions.
    - Map `updateMode` to `update_cadence` (see mapping above).
    - Call `batchControllerAssignments(privateBeachId, [{ controller_session_id, child_session_id, prompt_template, update_cadence }])`.
    - Optionally persist `pollFrequency` alongside relationship in layout (already serialized) for agent consumption.
  - Reconciliation loop:
    - On dashboard load and on any tile `sessionMeta.sessionId` change, scan all relationships whose endpoints now both have session ids and attempt `batchControllerAssignments` for any missing/failed pairs. This keeps CanvasLayout and manager pairings in sync after reloads or late attaches.
  - Conflict resolution:
    - If multiple edges target the same (agent, child) pair, treat the latest saved edge as authoritative (last-write-wins). Deduplicate when batching to manager.
  - Removal:
    - When an edge is deleted (or an agent/application tile detaches), call `DELETE /sessions/:controller_id/controllers/:child_id` to tear down the pairing and allow the agent SSE stream to emit a `removed` event. Also clear `sourceSessionId`/`targetSessionId` from the relationship metadata so reconciliation doesn’t recreate the pairing.

3) Trace Flag (Optional)
  - Add a “Trace communications” toggle per Agent tile and store in layout metadata (and/or a global toggle in the page). Thread it into API calls as `metadata.trace=true` on the agent session and include `trace_id` in any action payloads emitted by the dashboard in future.
  - Trace id policy: mint a per-agent `trace_id` (UUID) in the dashboard and store at `metadata.agent.trace.trace_id`. Include an `x-trace-id` header on manager API calls where helpful.
  - Manager logging: extend controller pairing + action routes (and SSE emitters) to log the `X-Trace-Id`/`trace_id` alongside each event so the dashboard, manager, and agent logs can be correlated.

## Agent (demo TUI) Changes
1) Pairing Event Subscription
  - Add a second SSE consumer to the agent:
    - `GET /sessions/:controller_id/controllers/stream`
    - Handle events of shape:
```json
{ "controller_session_id": "sess-agent",
  "child_session_id": "sess-child",
  "action": "added|updated|removed",
  "pairing": {
    "pairing_id": "…",
    "prompt_template": "…",
    "update_cadence": "fast|balanced|slow",
    "transport_status": { "transport": "pending|fast_path|http_fallback", "last_event_ms": 0, "latency_ms": 5 },
    "created_at_ms": 0,
    "updated_at_ms": 0
  }
}
```
    - On Added/Updated:
      - Fetch `GET /sessions/:child_id/state` once to obtain the latest `StateDiff` for bootstrap.
      - Begin `GET /sessions/:child_id/state/stream` for live updates.
      - Cache cadence choice (SSE default; if update mode is `poll`, schedule `GET /state` every N seconds as a fallback when SSE is unavailable).
      - Controller lease: pairing does not imply control. Ensure the agent holds/renews a controller lease via `POST /sessions/:controller_id/controller/lease`; on Added, attempt/refresh the lease.
    - On Removed: stop streaming/polling that child.

2) Prompt/Instructions Consumption
  - Read `prompt_template` from pairing events and/or the agent session’s metadata `prompt_pack` (via `GET /private-beaches/:id/sessions` and filtering by `session_id`).
  - Display in the agent TUI header/log; use it to set initial behavior.

3) Polling Strategy
  - Default to SSE (fast path if available) for child state.
  - If the edge’s `updateMode` was `poll`, respect the saved `pollFrequency` if SSE is unavailable or explicitly disabled. Schedule periodic `GET /sessions/:child_id/state` and treat it as a diff source.
  - Source of `pollFrequency`:
    - Near-term: the agent fetches `GET /private-beaches/:id/layout`, finds the relationship whose `sourceSessionId`/`targetSessionId` matches the `(controller_session_id, child_session_id)` pair, and reads its `pollFrequency` + `id`. This requires `pb:beaches.read` scope for the agent.
    - Future: manager may include `poll_frequency` in pairing SSE events to avoid layout reads by agents.

4) Tracing (Optional but Recommended)
  - When `trace` is enabled via the agent session’s metadata, include a `trace_id` on logs and mirror the manager’s logs with the same id. Log a compact JSON line for every control and state event.
  - Propagation: read `metadata.agent.trace.trace_id`; include in queued action payloads (e.g., `{ meta: { trace_id } }`) and attach as `X-Trace-Id` header on HTTP requests when present. The manager can log this id alongside `agent_controller_comms` traces.

## Security & Auth
- Dashboard needs a token with scopes:
  - `pb:beaches.read`, `pb:beaches.write` (layout)
  - `pb:sessions.read` (credentials, snapshots)
  - `pb:control.write` (create pairings)
- Agent needs `pb:sessions.read` (SSE state), `pb:control.write` (queue actions / leases), and `pb:beaches.read` (fetch CanvasLayout metadata for `pollFrequency` + relationship mapping).
- No controller lease is required for the dashboard to create pairings; the agent itself must hold/renew its own controller lease to send actions to children (already implemented in the demo agent).
 - Token sources:
   - Dashboard: Clerk-issued bearer with the above scopes.
   - Agent demo: environment-provided token (e.g., `PB_MCP_TOKEN`) with `pb:sessions.read` and `pb:control.write`. Document rotation for long-running demos.

## Minimal Implementation Steps (Checklist)
Dashboard (rewrite‑2):
1) In `FlowCanvas.tsx`, on `handleEdgeSave`, resolve `source`/`target` session ids from tile state and call `batchControllerAssignments` with mapped cadence and a `prompt_template` composed from Agent tile + edge instruction. On success, close the edge editor; on failure, surface an inline error.
2) In `TileFlowNode.tsx`, when saving an Agent tile that has `sessionMeta.sessionId`, call `onboard_agent` and persist the returned `prompt_pack` to the agent session via `updateSessionMetadata` (metadata.role='agent'). Keep the CanvasLayout agent metadata unchanged.
3) Add an optional “Trace” toggle and thread `trace` via session metadata and API calls (optional).
 4) Add a reconciliation effect that, on load and on tile session-id changes, batches controller assignments for any relationships whose endpoints are now attached.

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
 7) Failure drills:
    - Edge save when either endpoint has no session id → inline error, no pairing call.
    - `onboard_agent` 4xx/5xx → warning and retry affordance; tile state remains consistent.
    - Pairing batch call partial failures → display per-edge errors and schedule reconciliation retry.
    - SSE disconnects → agent retries with backoff; falls back to polling if configured.
    - Conflicting multiple edges → confirm last-write-wins behavior and deduplication in batch.

## Notes & References
- Dashboard attach + viewer: `apps/private-beach-rewrite-2/src/components/ApplicationTile.tsx`
- Edge UI & store: `apps/private-beach-rewrite-2/src/features/canvas/FlowCanvas.tsx`, `apps/private-beach-rewrite-2/src/features/tiles/store.tsx`
- Layout persistence: `apps/private-beach-rewrite-2/src/features/canvas/useTileLayoutPersistence.ts`
- Manager pairing APIs: `apps/beach-manager/src/routes/sessions.rs` and `apps/private-beach/src/lib/api.ts` (`createControllerPairing`, `batchControllerAssignments`)
- Agent demo: `apps/private-beach/demo/pong/agent/main.py`

## Implementation Status
- ✅ Relationships now carry `sourceSessionId`/`targetSessionId`, and FlowCanvas automatically batches controller pairings whenever an edge gains both session ids plus saved instructions.
- ✅ Edge deletions (or any relationship removal) call the manager’s delete-pairing endpoint and drop cached sync signatures so the dashboard and manager stay in sync.
- ✅ Agent tiles now call `/agents/onboard`, persist the returned prompt pack + trace settings into session metadata, and expose a trace toggle that threads `trace_id` through dashboard API calls (with `X-Trace-Id` headers logged by the manager).
- ✅ FlowCanvas groups controller assignments by trace id, and the Pong demo agent now listens to controller-pairing SSE events, fetches `pollFrequency` from the canvas layout, spins up HTTP pollers when required, and tags MCP actions/logs with the propagated trace id.
- ✅ The manager logs trace identifiers for both batch assignments and queue_action calls, and the Pong agent surfaces the prompt pack instructions provided during onboarding so operators can see the active mandate inside the TUI.

## Trace Monitor & Runtime Logging
- FlowCanvas surfaces a “Trace” button on edges sourced from tracing-enabled agents. Clicking it opens an overlay that shows the trace id, prompt summary, last assignment sync attempts, and a live stream of client-side trace logs for that id.
- `useSessionConnection` and the viewer connection service now include `trace_id` in the JSON payloads they print to the browser console whenever a tile connects/reconnects, making it trivial to filter logs per trace.
- Manager-side SSE endpoints (`/state/stream`, `/controllers/stream`) accept `X-Trace-Id`, log subscription + emission events with the supplied id, and `onboard_agent`/`list_sessions` responses record trace ids for auditing.

## Agent Prompt & Bridge Awareness
- The demo agent TUI exposes a prompt panel (toggle with `P`) that renders the manager-provided instructions verbatim and summarizes any structured directives (`serve_preference`, `paddle_strategy`, cadence overrides). Those directives directly influence autopilot behavior so prompt packs become operational rather than advisory.
- Available MCP bridges (e.g., `private_beach.subscribe_state`, `private_beach.queue_action`) are listed at the top of the TUI with live states such as `pending`, `streaming`, `sent`, or `error`. Bridge states update automatically when SSE streams connect or MCP actions succeed/fail.
- A `--headless` mode lets the agent run without curses for CI/automation. Headless runs still honor prompt directives and emit structured logs with the trace id.

## Automated Validation
Use `scripts/pong_showcase_validate.py` to spin up two player harnesses plus the agent, enable trace logging, draw edges, and verify controller/state SSE propagation end-to-end:

```bash
python3 scripts/pong_showcase_validate.py \
  --manager-url https://manager.private-beach.test/api \
  --private-beach-id <beach-id> \
  --auth-token $PB_MANAGER_TOKEN
```

On success the script reports the generated trace id along with the session ids it orchestrated. Failures exit non-zero with diagnostic output; all harness processes are cleaned up automatically.

## Implementation Progress
- 2025-11-07 16:59 EST — Updated `apps/private-beach/src/lib/api.ts` with the new `onboardAgent` helper signature and trace-aware headers; verified POST payload includes scoped roles plus default options.
- 2025-11-07 17:07 EST — Wired `TileFlowNode` + `AgentMetadata` for the new trace toggle and onboarding flow; confirmed saves flip roles, call `onboardAgent`, and stash prompt packs/profile data without blocking the UI.
- 2025-11-07 17:12 EST — Extended `FlowCanvas` edge saves and reconciliation to call `batchControllerAssignments` with prompt templates + cadence, deduping controller/child pairs via `controller|child` sets; verified console logs fire for both immediate saves and background resync attempts.
- 2025-11-07 17:18 EST — Updated tile persistence/store + the Pong agent to round-trip `agentMeta.trace` and read poll/trace settings from layout; checked layout hydration and the demo agent’s metadata fetch run through the new session→tile mapping.
- 2025-11-07 18:01 EST — `FlowCanvas` now auto-acquires controller leases before syncing assignments, clearing the “lease required” edge errors seen in Rewrite-2 (files: `FlowCanvas.tsx`); validated by re-saving agent→app edges and observing successful batch calls.
- 2025-11-07 18:04 EST — Fixed manager `controller_pairing` queries to alias origin-session columns so Returnings hydrate `ControllerPairingRow` correctly (file: `apps/beach-manager/src/state.rs`); retrying edge save no longer shows the “database error: controller_origin_session_id” banner.

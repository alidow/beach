# Feedback on `pong-showcase-wiring.md`

## High-Level Takeaways
- The plan captures the right manager APIs and desired UX, but several sequencing details are still implicit (e.g., how agent-role metadata is reconciled with the shared `ApplicationTile` attach flow in rewrite-2). Clarifying these now will prevent regressions when agent tiles start making real calls.
- Edge→pairing orchestration needs a lifecycle story that covers “edge exists before either tile is attached” and “layout reload” scenarios; otherwise relationships in `CanvasLayout.metadata.agentRelationships` will silently diverge from manager pairings once the dashboard reloads.
- The agent-side work assumes availability of pairing SSE and a place to read per-edge metadata (poll intervals, trace flags), yet those transport surfaces are not described end-to-end. Calling out the exact schemas and fallback sources will make the agent demo changes straightforward.

## Detailed Feedback

### 1. Agent Attach + Onboarding Flow (lines 7-27, 59-63)
- Saving the agent tile today still goes through `ApplicationTile`, which unconditionally calls `updateSessionRoleById(..., 'application', ...)` after `attachByCode`. The plan should spell out whether we override that behavior or run a second metadata update to flip the role to `'agent'` before calling `onboard_agent`, so we do not momentarily register the controller as an application.
- When `onboard_agent` fails (network/scope problems), we need a recovery UX: should the save be rejected, or do we persist the role/responsibility locally and retry later? Explicitly defining the failure mode will keep the tile state consistent with session metadata.
- Storing the returned `prompt_pack` “in session metadata” needs a schema commitment. Without a reserved namespace (e.g., `metadata.agent.prompt_pack`), later automation might overwrite the blob. Consider documenting the exact JSON path and how large blobs are handled (these packs can exceed Clerk metadata limits if we are not careful).

### 2. Edge Save → Manager Pairing (lines 28-66, 88-110)
- `handleEdgeSave` currently fires per edge; the plan should note whether we batch over every dirty relationship on persist, or only the single edited edge. Without a reconciliation loop, reloading the canvas after adding several edges will not recreate pairings that failed earlier.
- There is no catch-up path when an edge exists but either endpoint attaches later. Since layout persistence already stores these relationships, we likely need a background effect (on tile meta change) that retries `batchControllerAssignments` for all relationships whose endpoints have newly acquired `sessionMeta.sessionId`.
- Mapping only the agent role/responsibility + edge instructions into `prompt_template` is a good start, but we also need to confirm whether per-edge context (e.g., “this edge controls LHS paddle”) should be embedded consistently. Documenting a template helper or format will avoid ad-hoc string concatenation in `FlowCanvas`.

### 3. Polling + Cadence Semantics (lines 48-65, 95-110)
- The plan maps `poll` → `balanced`, which is counterintuitive if “poll” is supposed to represent slower, explicit pulls. Consider clarifying why `balanced` is the right cadence, or update the mapping to keep semantic parity (e.g., `poll` → `slow`) while still storing `pollFrequency` for the agent to enforce.
- `pollFrequency` is said to remain in layout metadata “for agent consumption,” but the agent process currently has no API to fetch that metadata. Do we expect the agent to call the private beach layout endpoint, or will the manager reflect the value into the pairing event? Document whichever mechanism we prefer so both dashboard and agent implementers know where to read/write it.
- When multiple edges connect the same agent↔child pair, which cadence wins? Defining conflict resolution (latest save vs. highest priority) avoids ambiguity once we allow multi-agent supervision graphs.

### 4. Agent Pairing Subscription + Bootstrap (lines 32-45, 98-120)
- The SSE endpoint `GET /sessions/:controller_id/controllers/stream` is referenced, but its payload schema isn’t described (fields, heartbeat cadence, retry guidance). Including an explicit example—even a short JSON stub—would help the agent engineer wire up parsing before touching the backend.
- The agent bootstrap steps mention fetching `/sessions/:child_id/state` once, but do not specify how to determine whether the agent already holds a controller lease for that child. Should the agent attempt to acquire a lease as part of handling the Added event, or does the manager implicitly grant control when the pairing is created?
- Trace propagation on the agent side depends on reading the agent session’s metadata. Please clarify whether the agent should poll `/sessions/:id` for metadata changes, subscribe to a separate SSE, or expect the trace flag to arrive inside the pairing event so it can be applied without extra queries.

### 5. Traceability Story (lines 33-34, 111-115)
- “Trace communications” is described as a per-agent toggle, but we need a concrete plan for generating and propagating a `trace_id`. Does the dashboard mint one per agent tile, or per edge? How is it injected into manager logs (headers vs. metadata field)? Detailing this now will keep the three log streams correlated without ad-hoc conventions later.
- Consider noting where traces are stored/visualized (console only, or persisted somewhere). Otherwise implementers may ship verbose logging without a way to consume it.

### 6. Security & Testing Coverage (lines 116-150)
- The scopes listed at the end cover write paths, but the agent’s SSE subscription also needs long-lived read tokens. Documenting the expected token source (Clerk? service token?) and rotation story would help whoever wires the demo harness into CI.
- The testing checklist is positive-path only. Add failure drills: missing session id on edge save, manager returning 4xx/5xx on onboarding, SSE disconnects, or mismatched cadences. These cases are where the UX will otherwise get stuck without actionable feedback.

## Suggested Next Steps
1. Extend the plan with explicit schemas for the new metadata (`prompt_pack`, `trace`, `pollFrequency`) and note how agents/readers discover them.
2. Define a reconciliation loop (either on dashboard load or via manager cron) so persisted relationships always backfill controller pairings when prerequisites become available.
3. Document the pairing SSE payload and lease acquisition expectations so the agent implementation can move in parallel with dashboard work.

## Second-Pass Findings (post-update review)

- **No manager-side teardown path (lines 96-117).** The dashboard work now covers creating/refreshing pairings and reconciling newly attached tiles, but it never describes how to delete pairings when a relationship edge is removed or when an agent tile detaches. Without a corresponding `delete` call, `ControllerPairing` rows will linger and the agent TUI will continue to receive `child_session_id` events for sessions it no longer manages.
- **Agent polling metadata is unmappable (lines 152-157).** The doc says the agent should read `metadata.agentRelationships[relId].pollFrequency` from the layout, but the pairing SSE payload doesn’t include `relId`, nor does the doc explain how to convert a `(controller_session_id, child_session_id)` pair back to the tile/relationship IDs. Spell out the mapping (e.g., by embedding tile IDs or relationship IDs in pairing metadata) or the agent cannot find the right `pollFrequency`.
- **Scope mismatch for layout reads (lines 152-157 vs. 163-172).** Poll-frequency fallback requires calling `GET /private-beaches/:id/layout`, which needs `pb:beaches.read`, yet the Security section still says the agent only needs `pb:sessions.read` and `pb:control.write`. Either grant agents `pb:beaches.read` or pick a different transport for per-edge metadata.
- **Session metadata writes can clobber `sessionMeta` (lines 96-104).** The plan now stores `prompt_pack` under `metadata.agent.prompt_pack` immediately after `ApplicationTile` has saved `metadata.sessionMeta`. If the follow-up `updateSessionMetadata` doesn’t merge in the latest tile metadata (e.g., viewport info), it can wipe out the `sessionMeta` blob that keeps the dashboard in sync. Call out the need to merge with `buildSessionMetadataWithTile` (or equivalent) before persisting.
- **Trace header lacks a server plan (lines 118-121 & 160-161).** The dashboard/workflow now emits `X-Trace-Id`, but there’s no mention of adding logging hooks on the manager to capture that header or to surface it in controller-event logs. Without the backend instrumentation, the trace toggle still won’t produce “coherent logs” as stated in the success criteria.

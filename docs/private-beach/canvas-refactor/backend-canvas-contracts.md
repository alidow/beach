# Private Beach Canvas — Backend Contracts & Persistence Buildout

_Owner: Backend-focused Codex instance. Keep this doc up to date — append progress entries to the log at the end._

## Objective
Replace the legacy grid layout storage with the new canvas graph (layout version 3), eliminate the existing 12-tile ceilings, and provide the server-side capabilities required for the React Flow canvas (batch controller assignment, z-order, groups, viewport state).

## Scope
- Persistent storage
  - Add a `canvas_layouts` data store (SQL table or column) keyed by beach.
  - Persist full canvas graph (`version`, `tiles`, `groups`, `agents`, `controlAssignments`, `viewport`, metadata).
  - Provide one-time migration tooling to transform any surviving v2 layouts into the canvas graph (if sample data needs to be preserved).
  - Remove the existing 12-item caps in API normalizers (`apps/private-beach/src/lib/api.ts`, Next API route, any manager constraints).
- APIs
  - Replace `getBeachLayout`/`putBeachLayout` payloads with the canvas graph contract; validate with JSON schema or typed guards.
  - Introduce a batch controller assignment endpoint that accepts tile/group assignments and returns per-session results.
  - Ensure Clerk/Gate auth requirements and rate limiting are updated for the new endpoints.
  - Update manager service (`apps/beach-manager`) to read/write the canvas graph (or delegate to the Next API if that stays canonical).
- Client plumbing
  - Update TypeScript API types (`apps/private-beach/src/lib/api.ts`) to reflect the new payloads, including optimistic typing for `controlAssignments`.
  - Remove grid-oriented fields (`x`, `y`, `w`, `h`, `gridCols`, etc.) from new responses (keep legacy parsing only if migration script needs it).
  - Expose helpers to fetch/save the canvas graph for the React Flow front end.
- Testing & validation
  - Add unit/integration coverage for the new persistence path and batch endpoint.
  - Seed fixture data that covers multiple groups, 50+ tiles, and various z-index setups.
  - Provide a smoke script/command for other contributors to sanity check the backend after changes (document under “Verification” below).

## Out of Scope (Handled by other tracks)
- React Flow rendering, node manipulation, and frontend state management (`canvas-surface-implementation.md`).
- Terminal measurement / host resize logic (`terminal-preview-integration.md`).
- Grouping behaviours and drag/drop UI semantics (`grouping-and-assignments.md`).
- Test harness orchestration and perf dashboards (`testing-and-performance.md`).

## Interfaces & Coordination
- **Data contract**: mirror the `CanvasLayout` structure defined in `canvas-refactor-plan.md` and ensure changes are communicated to the front-end team.
- **Batch assignment API**: coordinate request/response shape with the grouping track; expect inputs like `{ assignments: [{ controllerId, targetType, targetId }] }` and response payload with per-item success/failure.
- **Schema changes**: inform the testing/perf track whenever database migrations or new seed data are added so they can update CI setups.

## Deliverables Checklist
- [x] Database schema and migrations scripted/documented *(Drizzle 0002 + manager 20250107000000 `surfer_canvas_layout`).*
- [x] REST/GraphQL endpoints updated (server + Next.js proxy route) *(manager `GET/PUT /private-beaches/:id/layout`, Next `/api/canvas-layout/:id`).*
- [x] Batch controller assignment endpoint live *(manager `POST /private-beaches/:id/controller-assignments/batch`).*
- [x] TypeScript API client updated with new types *(CanvasLayout helpers + batch assignment client).*
- [x] Legacy layout write path disabled/deleted *(Next `/api/layout/:id` now returns 410; client migrations completed).*
- [x] Automated tests & fixtures added *(Vitest covers canvas API; note: requires optional Rollup native module `@rollup/rollup-darwin-arm64` to run locally).*
- [x] Verification steps documented (see below).

## Verification Steps (update as implementation lands)
1. `pnpm --filter apps/private-beach test -- --run canvas-layout` — run Vitest coverage for the Next.js canvas layout API route *(install `@rollup/rollup-darwin-arm64` if the optional module is missing).*
2. `cargo test -p beach-manager batch_controller_assignments_endpoint` — verify the manager batch assignment endpoint behaviour.
3. Manual canvas smoke:
   ```sh
   # GET existing layout (falls back to migration if empty)
   curl -sS http://localhost:3000/api/canvas-layout/<beachId> | jq '.'

   # PUT updated layout (example with one tile/agent)
   curl -sS -X PUT \
     http://localhost:3000/api/canvas-layout/<beachId> \
     -H 'content-type: application/json' \
     -d '{
       "version": 3,
       "viewport": { "zoom": 1, "pan": { "x": 0, "y": 0 } },
       "tiles": {"tile-1": {"id": "tile-1", "position": {"x": 120, "y": 80}, "size": {"width": 448, "height": 320}, "zIndex": 1}},
       "agents": {},
       "groups": {},
       "controlAssignments": {},
       "metadata": { "createdAt": 0, "updatedAt": 0 }
     }' | jq '.'
   ```
   *(Alternatively run `scripts/smoke_canvas_api.sh <beachId>`.)*
4. Manual batch assignment check (requires manager auth token in `$TOKEN`):
   ```sh
   curl -sS -X POST \
     http://localhost:8080/private-beaches/<beachId>/controller-assignments/batch \
     -H "authorization: Bearer $TOKEN" \
     -H 'content-type: application/json' \
     -d '{
       "assignments": [
         { "controller_session_id": "controller-1", "child_session_id": "child-1" },
         { "controller_session_id": "controller-1", "child_session_id": "missing-child" }
       ]
     }' | jq '.'
   ```

Follow-ups: seeded fixtures covering 50+ tiles/groups are still pending—coordinate with the testing/perf track to cover large-scene scenarios once their harness is ready. 

## Progress Log
_Append new entries; do not overwrite existing rows._

| Date (YYYY-MM-DD) | Initials | Update |
| ----------------- | -------- | ------ |
| 2025-10-30 | CX | Added `surfer_canvas_layout` table + migration; new Next API `GET/PUT /api/canvas-layout/:id` storing v3 graph; removed 12-tile caps in API normalizers; added manager batch endpoint `POST /private-beaches/:id/controller-assignments/batch`; updated TS client with `CanvasLayout` types + helpers and `batchControllerAssignments`; added smoke script `scripts/smoke_canvas_api.sh`. |
| 2025-10-31 | CX | Manager now serves CanvasLayout v3 (RLS-enabled `surfer_canvas_layout` table + batch endpoint wiring); Next `/api/layout` retired in favor of `/api/canvas-layout`; Private Beach client updated to persist CanvasLayout via `CanvasSurface`; added Vitest coverage for the canvas API and Rust test for batch assignments; verification steps updated. |

# Private Beach — Server Source of Truth (Critical Cleanup)

Status: NEXT ON DECK (blocking testing)

Goal
- Eliminate all LocalStorage as a data source. Postgres (via Beach Manager) is the single source of truth for private beaches, their metadata, and dashboard layout.

Scope (MVP to unblock testing)
- Backend (Manager):
  - CRUD for private beaches owned by the caller
  - List caller’s beaches
  - Layout persistence per beach (and optionally per account)
  - Keep existing session attach/list endpoints unchanged
- Frontend (Surfer, Next.js):
  - Remove LocalStorage for beaches (`pb.beaches`, `pb.manager`, `pb.road`) and layout (`pb.layout.*`)
  - Fetch beaches and layout from Manager
  - “Create beach” calls Manager; server generates beach id
  - Continue to use live Beach Road (`https://api.beach.sh`) for attach-by-code
  - Establish Drizzle ORM with a Postgres connection in `apps/private-beach` for any Surfer-specific persistence (e.g., saved tile layouts) that is not yet modeled by the Manager API.

Out of scope (later follow-ups)
- Full membership management UI (invite/roles)
- Production JWT minting for harness bridge tokens (replace dev UUID)
- Replace SSE `?access_token` with Authorization header polyfill (prod hardening)

DB and RLS changes
- Tables already exist: `private_beach`, `private_beach_membership`, `session`, `controller_event`, `session_runtime`.
- Current RLS gates `private_beach` by a per-request GUC `beach.private_beach_id`. That blocks listing across beaches.
- Proposed RLS additions (non-breaking, additive):
  - Introduce `beach.account_id` GUC for request-scoped account.
  - Add SELECT policies allowing rows where the caller is a member.

Proposed policy changes (sketch)
```
-- In addition to existing policy that gates by beach.private_beach_id
ALTER TABLE private_beach ENABLE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS private_beach_member_select ON private_beach;
CREATE POLICY private_beach_member_select ON private_beach
FOR SELECT
USING (
  EXISTS (
    SELECT 1 FROM private_beach_membership m
    WHERE m.private_beach_id = private_beach.id
      AND m.account_id::text = current_setting('beach.account_id', true)
      AND m.status = 'active'
  )
);

-- Similarly for private_beach_membership (SELECT limited to caller’s own rows)
DROP POLICY IF EXISTS private_beach_membership_select ON private_beach_membership;
CREATE POLICY private_beach_membership_select ON private_beach_membership
FOR SELECT
USING (
  account_id::text = current_setting('beach.account_id', true)
);
```

Manager API (new)
- `POST /private-beaches`
  - body: `{ name: string, slug?: string }`
  - behavior: create beach, set owner_account_id = caller, insert membership (owner)
  - returns: `{ id, name, slug, created_at }`
- `GET /private-beaches`
  - returns beaches where caller is a member (via new RLS)
- `GET /private-beaches/:id`
  - fetch metadata; server sets `beach.private_beach_id` GUC for the SELECT
- `PATCH /private-beaches/:id`
  - update name/slug/settings; GUC set to id
- `GET /private-beaches/:id/layout`
  - returns `{ preset, tiles }` (migrated from LS)
- `PUT /private-beaches/:id/layout`
  - upsert layout JSON (per beach; optional per-account variant later)

Notes
- For all requests, set `beach.account_id` GUC from the auth token (bypass maps to a sentinel or omitted).
- Keep existing per-beach GUC (`beach.private_beach_id`) for scoped updates.

Frontend changes
- Remove `apps/private-beach/src/lib/beaches.ts` (and all usage):
  - Replace with API:
    - list: `GET /private-beaches`
    - get: `GET /private-beaches/:id`
    - create: `POST /private-beaches`
    - layout: `GET/PUT /private-beaches/:id/layout`
- New Beach page:
  - Stop generating UUID on client; call POST; redirect to `/beaches/:id`.
- Beach Dashboard:
  - On load, `GET /private-beaches/:id`; if 404, show “Beach not found” and CTA to create.
  - Load layout from server; update layout via PUT on changes.
- Remove LocalStorage keys: `pb.beaches`, `pb.layout.*`, `pb.manager`, `pb.road`.

Defaults and env
- Keep `BEACH_ROAD_URL=https://api.beach.sh` by default (already updated in compose).
- Keep `NEXT_PUBLIC_ROAD_URL=https://api.beach.sh` (already updated in compose).
- Dev auth bypass remains for local; production uses Beach Gate JWT.

Migration strategy
1) Add RLS policies (membership-based SELECT) and `beach.account_id` handling in Manager.
2) Implement Manager endpoints above.
3) Frontend: switch to server APIs; remove LS usage (keep a small shim to read old LS once and prompt to migrate name only, then delete keys).
4) Verify attach flows end-to-end using live api.beach.sh sessions.

Acceptance criteria
- Visiting `/beaches` lists beaches from Manager (empty on first run).
- Creating a beach via UI results in a new row in Postgres; `GET /private-beaches/:id` returns it.
- Dashboard layout persists in DB; page reload reflects last saved layout.
- Attach-by-code works with live Road (api.beach.sh) for any created beach.
- No LocalStorage reads/writes except purely ephemeral UI (optional dev toggles).

Security hardening (follow-up, but planned)
- Remove SSE `?access_token` usage; use Authorization header.
- Replace dev “bridge tokens” with scoped JWT minted by Beach Gate.
- Disable `AUTH_BYPASS` outside local dev; enable FORCE RLS in CI.

Timeline (aggressive)
- Day 1–2: RLS extensions + Manager endpoints.
- Day 3: Frontend migration (list/create/get/update + layout), remove LS.
- Day 4: E2E test pass on live Road, docs updated, cut dev release.

Open questions
- Layout scope: per beach (global) vs per account? MVP: per beach; later: per account override.
- Slug management: allow rename with uniqueness? Enforce lowercased unique slug per org later.

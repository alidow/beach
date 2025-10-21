# Private Beach — Session Onboarding & Attach Plan (One‑Shot)

## Overview
Add sessions to a Private Beach via three intuitive flows:
- By Code: claim a public Beach session using `session_id` + short code.
- My Sessions: pick from active sessions owned by the logged‑in user.
- Launch New: start a new CLI (or Cabana) session pre‑bound to a private beach.

This plan specifies UI/UX, API contracts (Manager + Beach Road), data usage, security, and a single pass implementation plan that another engineer can execute end‑to‑end.

> **Update (Jan 2025):** The legacy HTTP bridge-token flow described below has been superseded by Manager's WebRTC viewer. `POST /private-beaches/:id/harness-bridge-token` and `Beach Road /join-manager` no longer exist; the remaining notes are kept for historical context until the onboarding UX is rewritten.

## Goals
- Zero‑trust attach: prove control (code) or ownership (Road) before mapping a session to a beach.
- Minimal friction for users: simple UI, copyable CLI, immediate appearance in the dashboard.
- Clean contracts: Manager is the source of truth for beach mappings; Beach Road remains the source of truth for session ownership and liveness hints.

## Non‑Goals
- Complex invite workflows (covered by share‑links elsewhere).
- Long‑lived bridge credentials (bridge tokens are short‑lived and single‑use).
- Historical playback; only live attach is considered here.

## Actors
- Human user in the Surfer UI (logged in via Beach Gate OIDC).
- Beach Manager (Rust control plane) enforcing access and persisting mappings.
- Beach Road (session server) providing ownership, verification, and harness nudge.
- Harness (Beach Buggy sidecar) that registers to Manager and streams state.

## UX Spec
Add Session Modal (from Sessions page)
- Tabs: By Code | My Sessions | Launch New
- By Code
  - Inputs: Session ID (UUID), 6‑digit code.
  - CTA: Attach
  - Feedback states: Verifying… → Attached | Invalid code | Session not found | Already attached
- My Sessions
  - Table: title, kind (terminal/cabana), status, started_at, last_seen.
  - Multi‑select; CTA: Attach X sessions
  - Empty state: “No active sessions — launch one from CLI” with quick link to Launch New.
- Launch New
  - Copyable command(s):
    - Terminal: `beach run --private-beach <beach-id> --title "My Session"`
    - Cabana: `beach cabana --private-beach <beach-id>` (later)
  - Notes: requires `beach login`; shows minimal help and link to docs.

## API Contracts

Manager (new)
- POST `/private-beaches/:private_beach_id/sessions/attach-by-code`
  - Auth: user JWT (Beach Gate), scopes: `pb:sessions.write`
  - Body: `{ session_id: string, code: string }`
  - Flow: verifies with Road; on success, persists mapping, emits controller_event `registered`/`attached`.
  - 200: `{ ok: true, attach_method: "code", session: SessionSummary }`
  - 409/404: `{ error }`

- POST `/private-beaches/:private_beach_id/sessions/attach`
  - Auth: user JWT, scopes: `pb:sessions.write`
  - Body: `{ origin_session_ids: string[] }`
  - Flow: for each id, validate ownership via Road; persist mapping if not present; nudge harness if needed.
  - 200: `{ attached: number, duplicates: number, errors?: Array<{id, error}> }`

- POST `/private-beaches/:private_beach_id/harness-bridge-token`
  - Auth: user JWT, scopes: `pb:sessions.write`
  - Body: `{ origin_session_id: string }`
  - 200: `{ token: string, expires_at_ms: number, audience: string }`
  - Notes: short‑lived (≤ 5m), scopes: `pb:sessions.register pb:harness.publish`, audience bound to (beach, session).

Beach Road (new/extended)
- POST `/sessions/:origin_session_id/verify-code`
  - Auth: user JWT
  - Body: `{ code: string }`
  - 200: `{ verified: boolean, owner_account_id: string, harness_hint?: object }`

- GET `/me/sessions?status=active`
  - Auth: user JWT
  - 200: `Array<{ origin_session_id, kind, title, started_at, last_seen_at, location_hint }>`

- POST `/sessions/:origin_session_id/join-manager`
  - Auth: user or manager service token (server‑to‑server)
  - Body: `{ manager_url: string, bridge_token: string }`
  - Response: `{ ok: true }`

Harness
- Registers to Manager using bridge token: `POST /sessions/register` (existing), with `private_beach_id`, `session_id` = origin id.

## Data Model & Storage
- Reuse `session` row in Manager with fields `private_beach_id`, `origin_session_id`, `created_by_account_id`.
- Optional: add `attach_method TEXT CHECK (attach_method IN ('code','owned','direct'))` to `session` to audit how mapping was established.
- Events: emit `controller_event` entries: `attached` (new type) + `registered` if needed.
- No new tables required; leverage existing RLS with `beach.private_beach_id` GUC.

## Security & Policy
- UI login required; Manager enforces per‑beach roles (admin/owner) for attach operations.
- By Code requires Road verification (short code TTL, rate‑limited, single‑use if possible).
- Bridge tokens are single purpose, short‑lived, scoped to (beach, session), and cannot attach to other beaches.
- All requests are audited via controller events; errors don’t reveal existence beyond what the code allows.

## Implementation Plan (One‑Shot)
1) Manager APIs
   - Add routes + handlers:
     - `POST /private-beaches/:id/sessions/attach-by-code`
     - `POST /private-beaches/:id/sessions/attach`
     - `POST /private-beaches/:id/harness-bridge-token`
   - Add service methods:
     - `attach_by_code(beach_id, session_id, code, requester)`
     - `attach_owned(beach_id, origin_ids[], requester)`
     - `mint_bridge_token(beach_id, origin_id, requester)`
   - Integrate Beach Road client (verify, list, nudge) and Beach Gate (JWT mint) helpers.
   - Emit `controller_event` on attach and on registration.
   - Optional migration: add `session.attach_method`.

2) Beach Road APIs
   - Implement `verify-code`, `me/sessions`, `join-manager` endpoints.
   - Store owner for each session; maintain active session index by account.
   - Implement code issuance/display in CLI/harness; short TTL + rate limit.

3) Surfer UI
   - Add Session modal with tabs.
   - By Code: form + POST to Manager; show success/error; on success, refresh sessions list and toast.
   - My Sessions: fetch from Road using user JWT; multi‑select; POST attach; update list.
   - Launch New: render copyable commands; if not logged in, show login prompt.
   - Sessions panel shows “Attached via …” badge; Add to canvas remains unchanged.

4) CLI
   - `beach login` persists Beach Gate token.
   - `beach run --private-beach <beach-id> [--title]` registers directly to Manager using user token; prints attach confirmation.
   - Emit short claim code for By Code flow when running without `--private-beach`.

5) Tests & Validation
   - Unit: Manager attach handlers (happy path + ownership/code errors).
   - Integration (dev): mock Beach Road client in Manager; simulate verify/list/nudge.
   - E2E (manual):
     1) By Code: run CLI producing code; use UI to attach; see session appear; tile add works.
     2) My Sessions: login; attach from list; see sessions appear.
     3) Launch New: copy command; session registers into beach; appears live.

## Failure Handling
- Code invalid/expired: 400 with neutral message; suggest re‑generate.
- Ownership mismatch: 403; mask details beyond necessary.
- Harness offline after attach: UI shows “waiting for harness” with retry; Manager still stores mapping.
- Bridge token failure: retry mint; backoff; surface error to UI.

## Rollout Notes
- Dev mode can stub Beach Road APIs (local mock) until real endpoints land.
- Keep AUTH_BYPASS for local testing; ensure scopes in prod.
- Document CLI and UI guidance in Surfer README.

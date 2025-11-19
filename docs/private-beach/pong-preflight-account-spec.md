# Pong Showcase Preflight & Diagnostics Spec

## Goal

Prevent silent failures in the Pong showcase (no ball motion, fast-path "counter mismatch", etc.) by proactively validating controller prerequisites and surfacing actionable errors before the stack runs. Today, missing seed data (e.g. host account) or misconfigured controller pairings only show up as downstream transport errors. We want deterministic checks in Beach Manager and the rewrite-2 UI that:

1. Detect misconfigured accounts / pairings / tiles as soon as the user tries to start the showcase.
2. Return structured error codes so the UI can explain what is wrong and how to fix it.
3. Block the showcase start when prerequisites are missing instead of letting the stack fail later.

## Manager changes

### 1. Controller lease guardrails

- **Endpoints**: `/sessions/{id}/controller/lease`, `/sessions/{id}/controller/handshake`, controller pairing APIs.
- **New behavior**:
  - After resolving the requesting account, verify the `account` row exists and `status = 'active'`.
  - When the lookup fails, return `409` with JSON `{ "error": "account_missing", "message": "controller account <id> is not registered in this cluster" }` (no FK violation leaks).
  - Log the same error under `target="controller.leases"` for observability.

### 2. Preflight diagnostics route

- **New route**: `GET /private-beaches/{id}/showcase-preflight` (auth: `pb:beaches.read`).
- **Checks** (per private beach):
  1. Host CLI account exists (configurable list of required account IDs).
  2. Required tiles (`agent`, `lhs`, `rhs`) exist in the layout metadata and are attached to this beach.
  3. Controller pairings exist (agent ↔ lhs/rhs) and are in `active` state.
  4. Optional: verify latest sessions have an active fast-path controller channel (emit warning if not yet connected).
- **Response**:
  ```json
  {
    "status": "ok" | "blocked",
    "issues": [
      {
        "code": "missing_account",
        "severity": "error",
        "detail": "account 0000… not seeded"
      },
      ...
    ]
  }
  ```
- **Blocking logic**: `status = "blocked"` whenever an issue has severity `error`. Warnings keep `status = "ok"`.
- The list of accounts checked can be overridden via the `PONG_SHOWCASE_REQUIRED_ACCOUNTS`
  environment variable (comma-separated UUIDs). It defaults to the seed host CLI account
  `00000000-0000-0000-0000-000000000001`.

### 3. Seed data update

- Ensure `config/dev-seed.sql` seeds the host CLI account (already done in code change) so `db-seed` always inserts it.
- For other required fixtures (e.g. default org membership), document them in the preflight response so local devs know what to backfill.

## Rewrite-2 UI changes

### 1. Showcase Start flow

- Before launching `pong-stack.sh` from the UI, call `showcase-preflight`:
  - Render a small checklist under the "Start Showcase" button.
  - If `status = "blocked"`, disable the button and show each issue with remediation text (e.g., "Run `db-seed` to add Host User").
  - Provide a "Re-run checks" action for when the user fixes the issue.

### 2. Tile error handling

- Update the agent tile controller handshake component to watch for `account_missing` / `pairing_missing` error codes from the manager and replace the generic fast-path errors with a banner that links to the docs.

## Implementation notes

- `showcase-preflight` should be extensible (more checks later). Model each check as a trait/struct with `code`, `description`, and `severity`.
- Cache the preflight result for a short TTL (e.g., 30s) to reduce load while the user repeatedly opens the drawer.
- For CLI users (scripts/pong-stack.sh), add a `--preflight` flag that calls the same route and prints the response before spawning hosts.

## Acceptance criteria

1. Manager returns structured errors when controller lease prerequisites fail.
2. `GET /private-beaches/{id}/showcase-preflight` reports missing accounts/tiles/pairings.
3. Rewrite-2 UI blocks the showcase when preflight is blocked and surfaces remediation steps.
4. Running `pong-stack.sh` manually prints the preflight diagnostics before launching.

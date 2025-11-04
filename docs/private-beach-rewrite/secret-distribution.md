# Private Beach Rewrite – Secret Distribution

_Last updated: 2025-11-03 by WS-B_

This note documents how WS-B is handling the `PRIVATE_BEACH_MANAGER_TOKEN` (and optional URL/JWT variants) requested by WS-A for server-side session fetching in the rewrite app.

## Overview
- The rewrite app (`apps/private-beach-rewrite/`) uses the existing REST endpoints via `listSessions`.
- Server-side fetches require a Clerk-issued manager token that mirrors the legacy dashboard’s expectations.
- To keep parity with the current app and support CI, we expose a single environment variable `PRIVATE_BEACH_MANAGER_TOKEN` (with `PRIVATE_BEACH_MANAGER_JWT` as a fallback if teams already rely on that naming).

## Local Development
1. Run `scripts/setup-private-beach-rewrite-env.sh` to scaffold `apps/private-beach-rewrite/.env.local` from the shared template.
2. Open `apps/private-beach-rewrite/.env.local` and paste the manager token (plus optional URL overrides). Set `BEACH_TEST_MODE=true` while local Clerk wiring is still in progress so the app falls back to the env token. Example:

```bash
PRIVATE_BEACH_MANAGER_TOKEN=sk_test_manager_token_here
PRIVATE_BEACH_MANAGER_URL=http://localhost:8080
BEACH_TEST_MODE=true
```

The token can be reused from the existing private-beach app setup; no additional scopes are required.

### Shared token drop (2025-11-03)
- `PRIVATE_BEACH_MANAGER_TOKEN=pbm_tok_cal6M_sb9Lx15KuY5Ethfw_5fXO2dctE23dY_3hp7fM`
- `PRIVATE_BEACH_MANAGER_URL=https://staging-manager.beach.sh` (optional override; default remains `http://localhost:8080`)

Use these values for local development unless you have project-specific overrides. Rotate as needed once production credentials are issued.

## CI / Automation
- Add `PRIVATE_BEACH_MANAGER_TOKEN` to the CI secret store (e.g., GitHub Actions repository secrets or your internal pipeline vault).
- Optional: add `PRIVATE_BEACH_MANAGER_URL` if CI needs to point at a staging manager instance rather than localhost.
- Before running `npm run build` / `next test`, call `scripts/ci-export-private-beach-rewrite-env.sh` so the pipeline writes `.env.local` for the rewrite app.
- Example GitHub-style job snippet:

```bash
export PRIVATE_BEACH_MANAGER_TOKEN="${{ secrets.PRIVATE_BEACH_MANAGER_TOKEN }}"
export PRIVATE_BEACH_MANAGER_URL="${{ secrets.PRIVATE_BEACH_MANAGER_URL }}"
export BEACH_TEST_MODE="true"
scripts/ci-export-private-beach-rewrite-env.sh
npm install --prefix apps/private-beach-rewrite
npm run --prefix apps/private-beach-rewrite build
```

## Verification
- `scripts/verify-private-beach-rewrite-ssr.ts` exercises the SSR fetch path against the configured manager; run via `npx tsx scripts/verify-private-beach-rewrite-ssr.ts` after seeding env vars to confirm connectivity.

## Ownership & Next Steps
- WS-B owns the documentation and bootstrap of these env vars.
- WS-A continues to own the server-side session fetch implementation and can rely on this contract immediately.
- If token rotation or multi-environment separation is needed, loop in WS-F (telemetry/release) to fold it into the rollout checklist.

Questions? Ping WS-B (Codex) in `docs/private-beach-rewrite/sync-log.md` or the shared channel.

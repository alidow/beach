# CLI Authentication Integration Plan

## TL;DR
- Preserve frictionless public-mode usage (`beach host|join`) with zero prerequisite login.
- Introduce an optional `beach login` (or `beach auth login`) flow backed by Clerk/Beach Gate device authorization, mirroring the Private Beach web stack.
- Load Clerk/Beach Gate configuration via `.env`/environment so local and CI runs share a single source of truth.
- Store tokens using the existing credentials store (`~/.beach/credentials`) and surface scoped access tokens transparently when private features are invoked.
- Deliver integration in incremental phases with mockable services and regression tests that work offline.

## Current State
- **CLI Auth Stack:** `apps/beach` already includes `BeachGateClient`, device-flow helpers, and a credential store with profile management (`AuthCommand::{Login,Logout,Status,SwitchProfile}`).
- **Default UX:** Hosting or joining a public session runs without auth. No Clerk calls are issued unless the user explicitly invokes `beach auth login`.
- **Private Mode:** Private Beach Manager expects Clerk-issued JWTs (`pb:*` scopes) but the CLI does not yet request or forward those tokens.
- **Env Handling:** The CLI does not currently load `.env`; web and manager services rely on `dotenvy`/`dotenv`.
- **Mocking:** `apps/beach-gate` supports `CLERK_MOCK=1`, but documentation for tying this into CLI smoke tests is missing.

## Goals
1. Ship an opt-in authentication workflow that unlocks Private Beach features without disrupting public-mode defaults.
2. Ensure all auth-configurable endpoints (Beach Gate, Clerk issuer/audience, scope templates) are sourced from environment variables for consistency.
3. Reuse profile storage to mirror AWS CLI ergonomics: multiple profiles, default selection, `--profile` override.
4. Provide end-to-end guidance—CLI + docker-compose—to exercise the flow locally with Clerk mock mode.
5. Add test coverage (unit + integration) to guard token refresh, profile persistence, and unauthenticated fallbacks.

## Non-Goals
- For now, do **not** mandate authentication for public hosts/joins.
- Do not implement full private session joining (Noise handshake, sealed signaling). The plan only prepares auth plumbing.
- Avoid bundling UI/Surfer changes; focus solely on CLI + supporting services.

## User Workflows
### Public Mode (Default)
- User runs `beach host` or `beach join <id/url>`.
- CLI skips Clerk entirely, continues existing behavior.

### Private Mode Preparation
1. User runs `beach login [--profile <name>] [--set-current]`.
2. CLI starts device authorization (using Beach Gate, backed by Clerk):
   - Prints verification URL + code.
   - Polls until approval, stores refresh/access tokens, entitlements, email metadata.
3. CLI caches active profile (`~/.beach/credentials` + optional `~/.beach/config`).

### Private Feature Invocation
- When a command requires private entitlements (e.g., `beach join private/...`):
  1. Lookup or refresh access token.
  2. Attach `Authorization: Bearer <token>` header to Manager/Road calls.
  3. Respect `--profile`/`BEACH_PROFILE` overrides; prompt helpful errors if not logged in.

## Configuration & Environment
- Introduce `dotenvy::dotenv().ok()` in `apps/beach/src/main.rs`.
- Expected env vars:
  - `BEACH_AUTH_GATEWAY` (default `https://auth.beach.sh`)
  - `BEACH_AUTH_SCOPE`, `BEACH_AUTH_AUDIENCE` (optional)
  - Clerk/Beach Gate mock helpers for local dev: `CLERK_MOCK=1`, `BEACH_GATE_PORT`, etc.
- Add sample `.env.cli` (document-only) illustrating local values.
- Update docs to highlight precedence (`--profile` > `BEACH_PROFILE` > stored current profile).

## Architecture Touchpoints
- **CLI Parser (`terminal/cli.rs`):** Expose `beach login` as alias for `beach auth login` (backwards compatible).
- **Auth Module (`auth/mod.rs`, `auth/gate.rs`, `auth/credentials.rs`):**
  - Ensure refresh flow surfaces entitlements for scope checks.
  - Add helpers to detect token expiry and perform silent refresh.
- **Transport Layers:** When private sessions are detected, obtain bearer token before contacting Manager/Road.
- **Logging & Telemetry:** Log auth state transitions (without leaking secrets); respect `--log-level`.

## Implementation Phases
| Phase | Scope | Deliverables |
|-------|-------|--------------|
| **0. Baseline Audit** | Verify existing `AuthCommand` paths and credential store behavior; document manual smoke test. | Notes in this plan; confirm no regression to public mode. |
| **1. Env Bootstrapping** | Add `dotenvy` init in CLI, document required env keys. | Code change + README snippet + test ensuring env vars load. |
| **2. CLI Command UX** | Introduce top-level `beach login` alias, clean up help text, confirm `--profile` guidance. | Clap updates + `cargo run -p beach login --help` snapshot. |
| **3. Token Retrieval Helpers** | Implement `auth::require_access_token(profile)` that refreshes if needed and surfaces helpful errors. | Unit tests with in-memory stores/mocks. |
| **4. Private Feature Hookups** | Wire bearer tokens into private Manager/Road calls (feature-gated behind `private` URLs). | Integration test hitting mock endpoints. |
| **5. Local Dev/Metrics** | Add optional `beach-gate` service to docker-compose (mock mode) and document running `CLERK_MOCK=1 cargo run -p beach login`. | Compose entry + doc updates. |
| **6. Testing & QA** | - Unit: credential store, refresh.<br>- Integration: login with mock, join private w/ token.<br>- Manual checklist: public run, login, private join. | Tests + checklist appended to docs. |

## Testing Strategy
- **Unit Tests (`apps/beach/src/tests/auth/`):**
  - `test_profile_storage_roundtrip`: store, reload, delete.
  - `test_access_token_refresh`: simulate expired token → refresh.
  - `test_unauthenticated_public_join`: ensure no auth error.
- **Integration Tests (`apps/beach/tests/`):**
  - Spin up mock Beach Gate (with `CLERK_MOCK=1`) using `tokio::test`.
  - Validate device flow happy path and failure modes (`authorization_pending`, `slow_down`, `denied`).
- **CLI Snapshots:** Use existing harness to capture `beach login --help` and `beach auth status`.
- **Manual QA Checklist:**
  1. `cargo run -p beach host` (no login) → should work.
  2. `CLERK_MOCK=1 cargo run -p beach login` → confirm token stored.
  3. Inspect `~/.beach/credentials`.
  4. Attempt private join with and without token.

## Local Dev Workflow
1. `cp .env .env.cli` (optional) and populate Clerk/Beach Gate mock values.
2. `docker compose up beach-gate` (new service) + existing manager/road stack.
3. Run `cargo run -p beach login` → Browserless device code flow prints mock URL.
4. Execute private command (future `beach join private.<host>/...`) to verify bearer injection.

## Risks & Mitigations
- **Token Leak Risk:** Ensure logs redact tokens; store files with restrictive permissions (`0600` already enforced).
- **Offline Behaviour:** Device flow requires network; provide mock mode instructions and guard CLI with clear errors when gateway unreachable.
- **Profile Drift:** Multiple profiles might point to different gateways; include gateway in profile name prompt and display during `status`.
- **Backward Compatibility:** Keep legacy `beach auth login` path; add tests verifying alias.

## Open Questions
1. Should we auto-detect private URLs (`private.` prefix) vs explicit flag (e.g., `--private`)? Decision pending UX input.
2. Do we need to support browser-based login (open URL automatically) in addition to device code? Investigate after base flow ships.
3. How should we surface entitlements (e.g., private beach vs fallback) in CLI status output?
4. Will the CLI need to cache Clerk ID tokens beyond Beach Gate access tokens (e.g., for Noise PSK plans)?

## References
- `apps/beach/src/auth/*` – existing credential and device flow logic.
- `apps/beach-manager/src/auth.rs` – how manager validates Clerk JWTs.
- `apps/private-beach/.env.local` – example Clerk keys for Surfer.
- `docs/private-beach/STATUS.md` – environment expectations for private stack.
- `docs/beach-transport-evolution-phases.md` – Phase 7 private mode roadmap. 


# Beach Gate – Auth Gateway Plan

## Purpose
- Provide a hardened gateway between Clerk and Beach services for issuing entitlements.
- Serve short‑lived, signed JWT access tokens to Beach clients while keeping Clerk keys server-side.
- Offer a consistent, AWS CLI–style credential experience (`beach login`, env vars, profile files).

## Scope & Goals
- Support device authorization flow via Clerk for the Beach CLI.
- Persist subscription tiers and feature entitlements fetched from billing.
- Validate access tokens on behalf of downstream Beach services (Beach Rescue, future Private Beach).
- Keep sensitive tokens off the CLI; use existing Beach transport encryption (no new TLS layer).

## Components
- **Beach Gate HTTP API (apps/beach-gate)**: Fastify-based Node service issuing tokens and entitlements.
- **Clerk Integration**: Device flow + session verification using Clerk secrets from environment.
- **Entitlement Store**: Read-mostly cache seeded from billing webhooks; simple in-memory map for the first cut.
- **Token Service**: Signs asymmetric JWTs (ES256) embedding entitlements and expiry.
- **Credential UX**: CLI reads `~/.beach/credentials`, env var overrides, and exposes `beach login/logout/status`.

## Key Flows
1. **Device Login**
   - CLI calls `POST /device/start` → Beach Gate proxies to Clerk device authorization endpoint.
   - CLI polls `POST /device/finish` with `device_code`; Gate exchanges with Clerk for session + refresh token.
   - Gate converts Clerk session to Beach refresh token (scoped) and returns to CLI.
2. **Access Token Refresh**
   - CLI submits refresh token to `POST /token/refresh`.
   - Gate loads entitlements, signs short-lived (5 min) JWT access token + 30 min refresh token.
3. **Entitlement Verification**
   - Services call `POST /authz/verify` with presented access token.
   - Gate validates signature/expiry, returns entitlements or 403.

All CLI/service traffic remains over HTTPS; Beach session data stays end-to-end encrypted per existing stack.

## API Surface (v0)
| Method | Path | Description |
| --- | --- | --- |
| `POST` | `/device/start` | Start Clerk device flow (returns `user_code`, `verification_uri_complete`, polling interval). |
| `POST` | `/device/finish` | Exchange `device_code` for Beach refresh token + initial access token. |
| `POST` | `/token/refresh` | Trade Beach refresh token for new access token pair. |
| `POST` | `/authz/verify` | Validate access token and return entitlements for downstream services. |
| `GET` | `/entitlements` | Authenticated endpoint returning the caller’s entitlements (CLI convenience). |

Authentication uses `Authorization: Bearer <token>` where applicable.

## Token Format
- Alg: ES256 (P-256) with private key stored as PEM on disk (`BEACH_GATE_SIGNING_KEY_PATH`).
- Payload fields: `sub`, `iss="beach-gate"`, `exp`, `iat`, `entitlements` array, `profile`, `tier`, optional `email`.
- Refresh tokens are opaque, SHA-256 hashed in-memory; persistent storage approach TBD.

## Credential Storage (CLI)
- Config path: `~/.beach/credentials` (TOML or ini-style with named profiles).
- Precedence: CLI flags > env vars (`BEACH_PROFILE`, `BEACH_ACCESS_TOKEN`, etc.) > profile file.
- Refresh tokens stored encrypted via OS keychain when available; fallback to locally encrypted blob w/ passphrase.
- Command UX: `beach login`, `beach logout`, `beach status`, `beach switch-profile`.

## Implementation Plan & Status
- [x] Scaffold Beach Gate service (TypeScript project, Fastify server, env config).
- [x] Implement Clerk device flow helpers (start/finish endpoints, env-driven).
- [x] Create JWT signer & refresh token manager (ephemeral store + ES256 keys).
- [x] Add entitlement lookup stub + middleware enforcing feature flags.
- [x] Provide verification and entitlements endpoints (baseline manual testing).
- [x] Add automated tests for auth flows and entitlement enforcement.
- [x] Document CLI integration points + future private beach entitlement requirements.

## Open Questions
- How will billing webhooks populate entitlements? (Future task: integrate billing service.)
- Should refresh tokens persist across restarts or use external store (Redis/Postgres)?
- Required scopes/claims from Clerk for subscription tiers—pending Clerk configuration.

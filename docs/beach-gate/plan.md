# Beach Gate – Auth Gateway Plan

## Purpose
- Provide a hardened gateway between the Beach Auth identity provider and Beach services for issuing entitlements.
- Serve short-lived, signed JWT access tokens to Beach clients while keeping upstream secrets server-side.
- Offer a consistent, AWS CLI–style credential experience (`beach auth login`, env vars, profile files).

## Scope & Goals
- Support the standard OIDC device authorization flow for the Beach CLI and other non-browser clients.
- Persist subscription tiers and feature entitlements fetched from billing.
- Validate access tokens on behalf of downstream Beach services (Beach Rescue, future Private Beach).
- Keep sensitive tokens off the CLI; rely on existing Beach transport encryption without introducing a new TLS layer.

## Components
- **Beach Gate HTTP API (apps/beach-gate)**: Fastify-based Node service issuing tokens and entitlements.
- **OIDC Identity Integration**: Device flow + session verification using Beach Auth client credentials supplied via environment.
- **Entitlement Store**: Read-mostly cache seeded from billing webhooks; simple in-memory map for the first cut.
- **Token Service**: Signs asymmetric JWTs (ES256) embedding entitlements and expiry.
- **Credential UX**: CLI reads `~/.beach/credentials`, env var overrides, and exposes `beach auth login/logout/status`.

## Key Flows
1. **Device Login**
   - CLI calls `POST /device/start` → Beach Gate proxies to the upstream device authorization endpoint.
   - CLI polls `POST /device/finish` with `device_code`; Gate exchanges for session + refresh token.
   - Gate converts the upstream session to a Beach-scoped refresh token and returns it to the CLI.
2. **Access Token Refresh**
   - CLI submits refresh token to `POST /token/refresh`.
   - Gate loads entitlements, signs short-lived (5 min) JWT access token + 30 min refresh token.
3. **Entitlement Verification**
   - Services call `POST /authz/verify` with the presented access token.
   - Gate validates signature/expiry, returns entitlements or 403.

All CLI/service traffic remains over HTTPS; Beach session data stays end-to-end encrypted per the existing stack.

## API Surface (v0)
| Method | Path | Description |
| --- | --- | --- |
| `POST` | `/device/start` | Start device flow (returns `user_code`, `verification_uri_complete`, polling interval). |
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
- Command UX: `beach auth login`, `beach auth logout`, `beach auth status`, `beach auth switch-profile`.

## Fallback Authorization Behavior
- WebSocket fallback is only attempted with a signed entitlement proof (`rescue:fallback`) when the client is already authenticated.
- The CLI and browser surface omit any network calls to Beach Gate unless the user has opted into Beach Auth and a profile is selected.
- When fallback transport is denied (no auth or missing entitlement), clients present a friendly explanation encouraging users to sign up at `https://beach.sh` for fallback access.
- Default behavior for unauthenticated users remains WebRTC-only; fallback is invisible unless credentials exist locally.

## Implementation Plan & Status
- [x] Scaffold Beach Gate service (TypeScript project, Fastify server, env config).
- [x] Implement device flow helpers (start/finish endpoints, env-driven).
- [x] Create JWT signer & refresh token manager (ephemeral store + ES256 keys).
- [x] Add entitlement lookup stub + middleware enforcing feature flags.
- [x] Provide verification and entitlements endpoints (baseline manual testing).
- [x] Add automated tests for auth flows and entitlement enforcement.
- [x] Document client integration points + entitlement requirements.
- [x] Wire `beach auth` CLI workflows to persist tokens/profiles without leaking secrets.
- [ ] Surface the same credential story in beach-web (PKCE + OIDC) and store proofs client-side.
- [x] Gate CLI fallback negotiation so proofs are only attached when a profile is active; skip otherwise.
- [x] Deliver polished denial messaging for unauthorized fallback attempts across CLI and web.

## Current Status
- Device authorization, token issuance, refresh rotation, and entitlement verification are all implemented behind Fastify endpoints.
- Service bootstraps without upstream credentials by using mock mode for local development.
- Vitest suite covers end-to-end happy paths, token rotation, entitlement protection, and invalid token rejection (`npm test` from `apps/beach-gate`).
- Local test run pending `npm install` (10s timeout encountered; rerun when convenient to verify).
- CLI now exposes `beach auth` login/logout/status commands, stores refresh tokens in the OS keychain (or passphrase-protected ciphertext), and only sends fallback entitlement proofs when a logged-in profile advertises `rescue:fallback`.
- beach-web surfaces Beach Auth call-to-action messaging when fallback is unavailable; PKCE login remains a follow-up item.

## Open Questions
- **Billing sync path** – Today entitlements come from config overrides only. Need decision on how billing updates will be delivered (e.g., webhook → Beach Gate API vs. direct database read) to plan storage and reconciliation.
- **Refresh token durability** – Refresh tokens live in memory and are invalidated on restart or across replicas. Confirm whether to accept that limitation for MVP or store tokens in Redis/Postgres for multi-node resilience.
- **OIDC claims** – Current device flow only retrieves basic profile info. Confirm whether the identity provider can supply subscription tier/entitlement claims directly, or if Beach Gate should treat it purely for identity and rely entirely on the billing feed.

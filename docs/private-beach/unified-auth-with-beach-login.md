# Unified Auth with `beach login` (Manager + Agent + UI)

Goal: use a single auth authority (Beach Gate) so `beach login` issues tokens that Manager trusts. No manual JWT pasting in production.

This document describes the minimal code/config changes to:

- Make Beach Manager verify Beach Gate tokens (instead of Clerk) via JWKS.
- Ensure Beach Gate issues tokens Manager understands (scopes and algorithm).
- Keep local dev working; enable a clean production path.

## Overview

Today
- Manager verifies Clerk JWTs via Clerk JWKS (RS256). It checks `scope`/`scp` for permissions.
- Beach Gate issues ES256 tokens with `entitlements` but no JWKS endpoint.
- The agent/CLI logs in against Beach Gate and receives a Gate token that Manager doesn’t accept.

Target
- Beach Gate publishes a JWKS endpoint for its signing key.
- Manager supports both RSA (RS256) and EC (ES256) JWKs and chooses the algorithm based on the JWT header or JWK `kty`.
- Beach Gate includes `scope`/`scp` claims (derived from entitlements) so Manager’s scope checks pass.
- docker-compose points Manager to Gate JWKS, with issuer `beach-gate` and audience `private-beach-manager`.
- Agent uses the CLI token (from `beach login`) directly for Manager calls.

## Changes Required

### 1) Beach Gate: Add JWKS and scope claims

Files: `apps/beach-gate`

1. Expose JWKS
   - Add a route `GET /.well-known/jwks.json` that returns a JWK set for the current signing key.
   - For ES256 (P-256) keys:
     - JWK fields: `kty: "EC"`, `crv: "P-256"`, `x`, `y`, `kid`.
     - Derive `x`/`y` from the public key and base64url encode.
   - Optional: support RSA in the future (`kty: "RSA"`, `n`, `e`).

2. Include `scope` (and `scp`) in access tokens
   - In `token-service.ts` `issueAccessToken`, compute:
     - `const scopes = entitlementsToScopes(context.entitlements)` → space-separated string
     - Set `scope: scopes` and `scp: context.entitlements` in the JWT payload (in addition to the existing `entitlements`).
   - Scopes typically needed by Manager:
     - `pb:sessions.read pb:sessions.write pb:beaches.read pb:beaches.write pb:control.read pb:control.write pb:control.consume pb:agents.onboard`
     - For harness upload paths: `pb:harness.publish`.

3. Configure Gate for Manager audience
   - Ensure `BEACH_GATE_SERVICE_AUDIENCE=private-beach-manager` in Gate’s environment (compose).

### 2) Beach Manager: Accept EC JWKS + alg selection

Files: `apps/beach-manager/src/auth.rs`

1. Fetch JWKS (unchanged) but support both RSA and EC JWKs.
   - When building a `DecodingKey`, branch on JWK `kty`:
     - RSA: use `DecodingKey::from_rsa_components(n, e)` and `Algorithm::RS256`.
     - EC (P-256): use `DecodingKey::from_ec_components(x, y)` and `Algorithm::ES256`.
   - Cache keys by `kid` (existing) regardless of type.

2. Pick validation algorithm dynamically
   - Read `alg` from `decode_header(token)` when available; fall back to inferred from JWK.
   - Set `validation = Validation::new(Algorithm::RS256 | ES256 as appropriate)`.

3. Scope checks: unchanged
   - Manager already checks `scope` and `scp` claims. Gate will now supply those.

### 3) docker-compose: Set Manager → Gate trust

File: `docker-compose.yml`

Under `beach-manager.environment` set:

```
BEACH_GATE_JWKS_URL=http://beach-gate:4133/.well-known/jwks.json
BEACH_GATE_ISSUER=beach-gate
BEACH_GATE_AUDIENCE=private-beach-manager
```

Under `beach-gate.environment` set:

```
BEACH_GATE_TOKEN_ISSUER=beach-gate
BEACH_GATE_SERVICE_AUDIENCE=private-beach-manager
# (optionally) BEACH_GATE_SIGNING_KEY_PATH=/secrets/beach-gate-ec256.pem
```

Restart stack after changes.

### 4) Agent launcher: no manual JWTs in prod

File: `apps/private-beach/demo/pong/tools/run-agent.sh`

- Already updated to use `PB_MANAGER_TOKEN` if provided; otherwise uses the CLI token.
- With Manager now trusting Gate, `beach login` → CLI token works everywhere. No overrides needed in prod.
- Script validates access to the target private beach before starting.

### 5) Frontend Rewrite v2: fetch Gate token server-side

To eliminate Clerk JWTs when calling Manager, the rewrite app now proxies through Beach Gate:

1. API route `apps/private-beach-rewrite-2/src/app/api/manager-token/route.ts` exchanges the caller’s Clerk session for a Gate-issued JWT (`POST /auth/exchange`), returning `{ token }`.
2. Server-rendered pages use `resolveManagerToken` to fetch a Gate token before calling Manager. Client components call `/api/manager-token` via `useManagerToken` whenever they need to refresh credentials.
3. Configure the rewrite service with `PRIVATE_BEACH_GATE_URL` (defaults to `http://localhost:4133`) so both SSR and API routes know how to reach Gate. Clerk remains the human-facing identity provider; Gate only signs the Manager-bound tokens.

## Rollout Plan

1. Implement Gate JWKS + scope claims; set Gate service audience to `private-beach-manager`.
2. Update Manager auth to support EC JWKS (ES256) and alg selection.
3. Update compose envs to trust Gate JWKS/issuer/audience.
4. Verify:
   - `beach login` → run agent without `PB_MANAGER_TOKEN` override.
   - UI (rewrite v2) still works with Clerk in dev; in prod, wire to Gate token API.

## Quick Validation Commands

- Manager JWKS fetch:
  `curl -s http://localhost:4133/.well-known/jwks.json | jq` (should return at least one `EC` JWK with `kid`)

- Gate token scope:
  Issue token via device flow or CLI; decode:
  `node -e "const t=process.argv[1]; console.log(JSON.parse(Buffer.from(t.split('.')[1],'base64url')));" <eyJ...>`
  Ensure it includes `scope` and/or `scp` with `pb:*` scopes.

- Agent discovery:
  With only `beach login`, run agent and confirm it lists/controls sessions without 401/403.

## Appendix: Scope Mapping

Map entitlements to scopes verbatim (one-to-one). Minimal set for Pong:

```
pb:sessions.read pb:sessions.write pb:beaches.read pb:beaches.write pb:control.read pb:control.write pb:control.consume pb:agents.onboard pb:harness.publish
```

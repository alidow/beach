# AGENTS Guidance

This file captures repo-wide tips for coding agents (Codex, Claude, etc.).

## Docker + direnv

- The repo uses `.envrc` to populate WebRTC NAT hints (`BEACH_ICE_PUBLIC_IP` /
  `BEACH_ICE_PUBLIC_HOST`). Docker Compose commands must run with direnv-loaded
  env vars, otherwise `docker compose logs beach-manager` fails with
  `required variable BEACH_ICE_PUBLIC_HOST is missing a value`.
- One-time: run `direnv allow` in the repo root so the shell hook trusts
  `.envrc`. See `docs/helpful-commands/direnv-setup.md` for details.
- When executing Compose from scripts or non-interactive shells, prefix the
  command with `direnv exec .`, e.g. `direnv exec . docker compose logs
  beach-manager`. This mirrors how we resolved the error above and ensures
  every agent gets the right env without manual exports.

## Auth north star

- Authenticate external requests with Clerk, authorize controller access with
  Beach Gate. Manager should accept Clerk-issued JWTs (Clerk JWKS/issuer/aud
  must be set in env) and only rely on Beach Gate for controller token
  issuance/verification after auth succeeds. If you see `jwks missing requested
  kid ... kid=ins_*`, the manager is still using Beach Gate’s JWKS for auth and
  needs its Clerk config populated.

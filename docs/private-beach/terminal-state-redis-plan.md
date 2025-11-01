# Terminal State Storage: Redis-Only Plan

## Why change?
- We currently persist every `terminal_full` payload to both Redis and Postgres (`session_runtime.last_state`).
- Postgres writes happen on *every* host frame, creating needless write load and replication churn while providing only a cold-start preview.
- Redis is already the authoritative stream for live viewers and stores the latest frame per session, so we can eliminate the redundant SQL path.

## Objectives
- Treat Redis as the single source of truth for terminal state snapshots.
- Remove `last_state` writes/reads from the manager and dashboard code paths.
- Keep the dashboard fast on cold load by reusing the Redis snapshot API the manager already exposes.

## Implementation Notes
- Manager changes:
  - Delete the `record_state` branch that updates `session_runtime.last_state`.
  - Remove the column from schema/migrations and any SQL queries that select it.
  - Provide a lightweight HTTP endpoint that returns the cached Redis snapshot for a session (or reuse the existing Redis helper).
- Dashboard changes:
  - Stop deserializing `last_state` from session summaries.
  - Fetch the initial diff via the new Redis-backed endpoint when hydrating a tile.
- Operational impact: none. Redis already contains the payload; eliminating SQL writes reduces latency and disk usage.

## Rollout
- Update manager first (writes are idempotent, so a rolling deploy is safe).
- Once the dashboard relies solely on Redis, delete `last_state` data + column in Postgres.

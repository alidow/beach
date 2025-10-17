# Beach Manager (Control Plane)

## Purpose
- Serve as the orchestration layer for Private Beach: session registry, controller leases, shared state APIs, automation hooks, and billing integration.
- Provide authenticated REST + MCP endpoints consumed by Private Beach Surfer clients and automation agents.

## Planned Stack
- Rust (Tokio + Axum) for async HTTP/WebSocket workloads.
- Redis cluster for state cache/pub-sub, Postgres for durable metadata (`docs/private-beach/data-model.md`).
- WebRTC negotiation helpers for direct manager↔harness data channels.
- Tracing + metrics exported via OpenTelemetry.

## Initial Tasks
1. Boot minimal Axum server with health check + OpenAPI stub.
2. Implement Beach Gate JWT verification middleware.
3. Add Postgres connection pool (SQLx or SeaORM) and wire basic schema migrations.
4. Prototype session registry endpoints (`GET /private-beaches/:id/sessions`).

## Development
- `docker compose up postgres redis` to launch local dependencies (Postgres + Redis).
- Set `DATABASE_URL` (e.g. `postgres://postgres:postgres@localhost:5432/beach_manager`) and `REDIS_URL` before running the manager.
- `sqlx migrate run --source apps/beach-manager/migrations` applies schema changes.
- `cargo run -p beach-manager` starts the local server (development mode).
- `cargo test -p beach-manager` runs unit/integration tests.
  - To run the Postgres-backed SQLx test: start Postgres, export `DATABASE_URL`, then `cargo test -p beach-manager -- --ignored postgres_sqlx_e2e`.

## Directory Layout (Draft)
- `src/main.rs` – entrypoint + Axum router.
- `src/config.rs` – environment + CLI configuration.
- `src/routes/` – HTTP/MCP route handlers.
- `src/services/` – domain logic (sessions, controllers, shared state).
- `src/telemetry.rs` – tracing, metrics, structured logging helpers.

## Notes
- Follow the zero-trust principle: all APIs require mutually authenticated tokens; audit every controller change.
- Keep transport-agnostic modules (harness adapters, state codecs) in shared crates for reuse by CLI/agents.
- See `docs/private-beach/beach-manager.md` for detailed architecture and flows.

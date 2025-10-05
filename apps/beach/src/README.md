# Beach App Code Layout

This crate is organised around the lifecycle of a Beach state-sync application. Each top-level directory owns a single concern, with terminal-specific logic scoped inside a `terminal/` submodule so that future state machines (e.g. GUI) can plug in beside it.

- `cache/` — runtime caches that hold local copies of shared state. Terminal grids live in `cache/terminal/`.
- `client/` — client executables and presentation logic (TUI, input handling, predictive echo). Terminal joins live in `client/terminal/`.
- `mcp/` — Model Context Protocol bridges and adapters that expose Beach sessions to external tools.
- `model/` — value objects representing synchronised state and diffs.
- `protocol/` — wire-format definitions and helpers for frames, bootstrap envelopes, and feature flags.
- `server/` — host-side orchestration, PTY runtimes, and viewer management. Terminal hosting lives in `server/terminal/`.
- `session/` — broker interactions, session registration, authorisation, and shared UX helpers (e.g. join prompts).
- `sync/` — synchronisation pipelines, delta streams, backfill schedulers, and prioritised lanes.
- `telemetry/` — logging, metrics, and performance guards.
- `transport/` — transport abstractions (WebRTC, WebSocket, IPC, SSH bootstrap) and supervision utilities.
- `lib.rs` — module wiring for the crate; keep it lean and re-export only stable APIs.
- `main.rs` — CLI entry point that selects the appropriate state-sync implementation (currently terminal).

## Conventions

- Place reusable/generic code at the parent directory level; put implementation-specific code under `*/terminal/` (or another machine name).
- Keep `main.rs` thin: argument parsing, logging setup, and delegation only.
- Prefer cross-module APIs over deep imports to maintain clear dependency direction (`main` → `terminal/app` → `[client|server|transport|sync]`).
- Every structural change should include a deterministic test (`cargo test -p beach …`) before moving on.

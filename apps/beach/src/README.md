# Beach App Code Layout

This crate is organised around the lifecycle of a Beach state-sync application. Each top-level directory owns a single concern, with terminal-specific logic scoped inside a `terminal/` submodule so that future state machines (e.g. GUI) can plug in beside it.

## Directory Structure

- `cache/` — Runtime caches that hold local copies of shared state. Terminal grids live in `cache/terminal/`.
- `client/` — Client executables and presentation logic (TUI, input handling, predictive echo). Terminal joins live in `client/terminal/`.
- `debug/` — Diagnostic infrastructure for inspecting running sessions via IPC (Unix domain sockets).
- `mcp/` — Model Context Protocol bridges and adapters that expose Beach sessions to external tools.
- `model/` — Value objects representing synchronised state and diffs.
- `protocol/` — Wire-format definitions and helpers for frames, bootstrap envelopes, and feature flags.
- `server/` — Host-side orchestration, PTY runtimes, and viewer management. Terminal hosting lives in `server/terminal/`.
- `session/` — Broker interactions, session registration, authorisation, and shared UX helpers (e.g. join prompts).
- `sync/` — Synchronisation pipelines, delta streams, backfill schedulers, and prioritised lanes.
- `telemetry/` — Logging, metrics, and performance guards.
- `terminal/` — Terminal-specific CLI wiring, argument parsing, and application orchestration.
- `transport/` — Transport abstractions (WebRTC, WebSocket, IPC, SSH bootstrap) and supervision utilities.
- `lib.rs` — Module wiring for the crate; keep it lean and re-export only stable APIs.
- `main.rs` — CLI entry point that selects the appropriate state-sync implementation (currently terminal).

## Conventions

- Place reusable/generic code at the parent directory level; put implementation-specific code under `*/terminal/` (or another machine name).
- Keep `main.rs` thin: argument parsing, logging setup, and delegation only.
- Prefer cross-module APIs over deep imports to maintain clear dependency direction (`main` → `terminal/app` → `[client|server|transport|sync]`).
- Every structural change should include a deterministic test (`cargo test -p beach …`) before moving on.

## Runtime Diagnostics

The `debug/` module provides IPC-based diagnostic tools for inspecting running Beach sessions. This is useful for debugging state synchronization issues, cursor positioning, viewport scrolling, and cache integrity.

### Architecture

- `debug/mod.rs` — Protocol definitions for diagnostic requests and responses (serialized via serde)
- `debug/server.rs` — Unix domain socket server that handles diagnostic requests from running clients
- `debug/ipc.rs` — Client-side IPC helpers for sending requests to running sessions
- `client/terminal/debug.rs` — CLI command handler for the `beach debug` subcommand

### Usage

The `beach debug <SESSION_ID>` command queries runtime state from an active session via Unix domain sockets (located at `/tmp/beach-debug-<session_id>.sock`).

#### Query Types

**All State (default):**
```bash
beach debug <SESSION_ID>
# or explicitly:
beach debug <SESSION_ID> --query all
```

**Cursor State:**
```bash
beach debug <SESSION_ID> --query cursor
```
Shows:
- Cursor position (row, col)
- Sequence number (0 = no cursor frames received yet)
- Visibility and authoritative state
- Cursor support flag (whether server is sending cursor frames)
- Actual rendered cursor position in TUI (None = off-screen/invisible)

**Terminal Dimensions:**
```bash
beach debug <SESSION_ID> --query dimensions
```
Shows grid size and viewport dimensions.

**Cache State:**
```bash
beach debug <SESSION_ID> --query cache
```
Shows grid cache size, row offset, and first/last row IDs.

**Renderer State:**
```bash
beach debug <SESSION_ID> --query renderer
```
Shows:
- Cursor position in renderer
- Base row and viewport top
- Cursor viewport position (determines TUI visibility)

### Example Diagnostic Session

```bash
# Start a session via SSH
beach ssh user@host

# In another terminal, query the cursor state
beach debug <SESSION_ID> --query cursor

# Output:
# === Cursor State (Client Cache) ===
#   Position:      row=0, col=15
#   Sequence:      42
#   Visible:       true
#   Authoritative: true
#   Cursor support: true (server IS sending cursor frames)
#
# === Renderer State (What's Actually Rendered) ===
#   Cursor:        row=0, col=15
#   Cursor visible: true
#   Base row:      0
#   Viewport top:  0
#   Cursor viewport pos: (15, 0) - VISIBLE IN TUI
```

### Implementation Notes

- The diagnostic server only runs for SSH-initiated sessions (not join sessions)
- The Unix socket is created in `/tmp/` when the session starts and cleaned up on exit
- Requests are serialized as JSON and sent over the socket
- The diagnostic server runs in the client's event loop and responds without blocking UI rendering

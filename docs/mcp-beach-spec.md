# Beach MCP Integration Specification

## 1. Goals
- Expose Beach session capabilities through an MCP-compliant server so MCP clients (agents, IDEs, automation) can observe and control sessions.
- Preserve clear separation between transport-agnostic terminal logic and MCP glue code. Terminal-specific logic should live in `apps/beach/src/mcp/terminal` to allow future non-terminal surfaces.
- Provide an ergonomic developer experience: simple CLI entry point, documented resources/tools, and sensible defaults (localhost, read-only).

## 2. Non-Goals
- Supporting remote/non-local sockets in the initial release (future work via TLS/SSH tunnels).
- Providing image/video streaming. Initial release focuses on text-based terminal data.
- Low-level PTY emulation changes. MCP integration should reuse existing sync/input plumbing.

## 3. Architecture Overview
```
apps/beach/src/mcp/
  mod.rs              // top-level server wiring, configuration, feature toggles
  server.rs           // MCP server runtime (transports, JSON-RPC framing, routing)
  protocol.rs         // Shared request/response helpers, schema serialization
  auth.rs             // Token + lease management
  state.rs            // Session catalog abstraction (traits over available surfaces)
  terminal/
    mod.rs            // Terminal-specific glue implementing trait contracts
    resources.rs      // Resource producers for snapshots/deltas
    tools.rs          // Input/viewport/resize tool implementations
    events.rs         // Delta stream subscription handling
```

### 3.1 Integration Points
- **Session Introspection**: `SessionManager` / `HostSession` in `apps/beach/src/session` exposes active sessions, grids, and writer handles. We add read-only adapters for MCP.
- **Terminal Data**: reuse `TerminalSync`, `TerminalDeltaStream`, and the shared `TerminalGrid` cache. Terminal adapters translate these to MCP resource payloads.
- **Input Path**: use existing input encoding (see `TerminalClient::send_input_internal`) and the server-side PTY writer to route `send_text`/`send_keys` tool calls.
- **Event Loop**: run an async task (Tokio) per MCP connection. Use JSON-RPC 2.0 over stdio (sidecar mode) or a Unix domain socket (default `~/.beach/mcp/<session-id>.sock`, emitted in the host banner so multiple hosts can coexist).

### 3.2 Separation of Concerns
- `mcp` module owns protocol framing, server lifecycle, routing, authorization.
- `mcp::terminal` implements the `McpSurface` trait that exposes terminal-specific resources and tools.
- Additional surfaces (e.g., copy-mode, file browser) would add new submodules under `mcp/`.

## 4. Protocol Surface
Beach MCP server implements the MCP spec (JSON-RPC 2.0). Notable methods/resources:

### 4.1 Session Discovery
- `mcp/ships-capabilities` (standard) includes Beach-specific capability flags: `"terminal:grid"`, `"terminal:input"`, `"terminal:cursor"`.
- `resources/list` returns resources:
  - `beach://session/<id>/terminal/grid` (kind `terminal.grid`)
  - `beach://session/<id>/terminal/cursor`
  - `beach://session/<id>/terminal/history`
- `beach.sessions.list` tool (non-standard convenience): returns structured session metadata (id, label, role, capabilities, history_rows, active clients).

### 4.2 Resources
Resources respond to `resources/read` and can be subscribed via `resources/subscribe`.

#### `terminal.grid`
- **Read** payload:
  ```json
  {
    "cols": 120,
    "rows": 40,
    "base_row": 24010,
    "viewport": {
      "top": 23970,
      "height": 40
    },
    "lines": [
      {
        "row": 23970,
        "text": "top line",
        "cells": [ {"ch": "t", "style": 0}, ... ]
      },
      ...
    ],
    "cursor": {"row": 23985, "col": 4, "seq": 812344, "visible": true}
  }
  ```
- **Subscription events**: `terminal.grid.delta` notifications containing:
  ```json
  {
    "base_row": 24012,
    "watermark": 812355,
    "updates": [
      {"type": "row", "row": 24012, "cells": "..."},
      {"type": "cursor", "row": 24012, "col": 4, "seq": 812355}
    ]
  }
  ```
- Internally derived from `TerminalDeltaStream` for the requested viewport lane.

#### `terminal.history`
- Read-only range query parameters: `start_row`, `count<=500`, optional `mode` (`ansi`|`json`).
- Serves archived rows using `TerminalGrid::snapshot_row_into`.

#### `terminal.cursor`
- Lightweight read returning current cursor info (for clients wanting quick polling without full grid).

### 4.3 Tools
Tools follow MCP `callTool` semantics.

| Tool | Description | Params |
|------|-------------|--------|
| `beach.sessions.list` | Enumerate active sessions | `{ "filters": {"role": "host|client"} }` |
| `beach.terminal.acquireLease` | Acquire control lease | `{ "session_id": "...", "scope": "input", "ttl_ms": 30000 }` |
| `beach.terminal.releaseLease` | Release lease | `{ "lease_id": "..." }` |
| `beach.terminal.sendText` | Send literal text (paste) | `{ "session_id": "...", "text": "...", "lease_id"? }` |
| `beach.terminal.sendKeys` | Send structured keys | `{ "session_id": "...", "keys": [{"kind": "named", "name": "Enter"}, {"kind": "char", "char": "a", "modifiers": ["ctrl"]}], "lease_id"? }` |
| `beach.terminal.resize` | Adjust PTY size | `{ "session_id": "...", "cols": 120, "rows": 32 }` |
| `beach.terminal.setViewport` | Hint desired viewport | `{ "session_id": "...", "top": 24000, "rows": 40 }` |
| `beach.terminal.requestHistory` | Force history backfill | `{ "session_id": "...", "start_row": 23800, "count": 120 }` |

### 4.4 Authorization & Leases
- Server can be launched read-only by default (`--mcp-readonly`).
- Tools that modify state (sendText, sendKeys, resize, setViewport) require an active lease.
- `acquireLease` returns `{ "lease_id": "uuid", "expires_at": "..." }`. TTL defaults to 30s, auto-renew on successful tool calls.
- Only one write lease per session; read subscriptions do not require leases.

## 5. CLI Integration
Add `beach mcp` subcommands:

```
beach mcp serve [--socket <path>] [--stdio] [--read-only] [--allow-write]
               [--session <id>] [--token <path>] [--no-terminal]
```

- Default mode listens on `~/.beach/mcp.sock` (error if file exists).
- `--stdio` runs a single connection sidecar on stdin/stdout (for spawning from MCP-enabled tools).
- `--session <id>` restricts surface to a single session (otherwise exposes all host sessions).
- `--read-only` is implied unless `--allow-write` or `BEACH_MCP_ALLOW_WRITE=1`.
- Token file (JSON with array of api tokens) used when non-local clients are supported; currently optional placeholder.

Bootstrap integration: add `BEACH_MCP_AUTOSTART=1` to start the server alongside `beach host`.

## 6. Runtime Components
- **`McpServer`**: handles listener (Unix socket/stdio), accepts connections, spawns `McpConnection`.
- **`McpConnection`**: jsonrpc loop with `call`, `response`, `notification` handling; uses `serde_json::Value`.
- **`SurfaceRegistry`**: resolves requested resource or tool to a concrete surface implementation (terminal for now).
- **`TerminalSurface`**:
  - Maintains view over `TerminalGrid` and `TerminalDeltaStream`.
  - Implements `ResourceProvider` (snapshot/delta) and `ToolProvider` (input, viewport, etc.).
  - Manages per-connection viewport subscriptions with coalescing/backpressure.
- **`LeaseManager`**: ensures exclusive control for write tools; integrates with `tokio::time::Instant` for expiry.

## 7. Implementation Plan

### Phase 1 – Docs & Scaffolding
- [x] Write spec (this document).
- Create `apps/beach/src/mcp` tree with module stubs and trait definitions.
- Implement CLI plumbing (`beach mcp serve`) returning `unimplemented!()` placeholders.

### Phase 2 – Core MCP Server
- Implement JSON-RPC framing over stdio/unix socket with Tokio.
- Provide connection management and method routing.
- Implement `resources/list`, `resources/read`, `resources/subscribe`, `resources/unsubscribe`, `tools/list`, `ping`, `shutdown`.
- Expose session listing using `SessionManager`.

### Phase 3 – Terminal Surface Read-Only
- Implement `TerminalSurface` adapters for:
  - Grid snapshots (`terminal.grid` resource),
  - Cursor resource,
  - History resource,
  - Delta subscription (Foreground lane only initially).
- Add feature flags & capability advertisement.

### Phase 4 – Write Tools & Leases
- Add lease manager with `acquireLease`/`releaseLease` tools.
- Implement `sendText` (raw bytes) and `sendKeys` (key mapping) reusing existing encoders.
- Wire `TerminalRuntime` input channel to apply writes.
- Add optional `--allow-write` flag gating registration of write tools.

### Phase 5 – Advanced Controls & Polish
- Implement viewport hinting, history request, resize.
- Support per-subscription lane configuration (Foreground/Recent/History).
- Add structured logging, metrics counters.
- Provide CLI autostart integration.

### Phase 6 – Testing & Documentation
- Unit tests for JSON-RPC framing, lease manager, key encoder parsing.
- Integration tests using mock session + virtual terminal to assert resource/tool flows.
- Update docs with usage examples and CLI reference.

## 8. Open Questions / Future Work
- Multi-tenant security (tokens, TLS). For now restricted to local socket/stdio.
- Binary protocol bridging vs. JSON payload size (may introduce optional compression later).
- Recording/replay via MCP as part of CI harness.

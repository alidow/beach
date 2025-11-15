## Controller IPC Bridge Plan

### Goal
Let any CLI process running **inside** a Beach host session (e.g. the Pong agent, Claude, Bash) hand controller actions to the local harness instead of POSTing directly to Beach Manager. The harness should reuse its existing fast-path transport (mgr-actions data channel) and fall back to HTTP seamlessly, so controller traffic automatically benefits from the same fast-path guarantees as host traffic. The interface must also power a `beach action` command that is ergonomic, fast, and capable of targeting a specific local host session ID.

### High-Level Architecture
1. **Host additions (apps/beach/src/server/terminal/host.rs)**  
   - Extend `ControllerActionContext` so it exposes a method that accepts a serialized `ActionCommand` plus metadata (controller token, trace id, lease id). This method wraps the existing queue pipeline (fast-path first, HTTP fallback, logging, ack injection).
   - Teach the host to start an **IPC bridge** alongside the MCP server. The bridge will live in the existing MCP stack so tools can invoke it over either stdio or Unix socket. We will add new MCP tools under `controller.*`.
   - Track controller readiness/authorization inside the host: reuse `LeaseManager` so an IPC call must first acquire a controller lease (this also gives us the controller token to use when talking to the manager).

2. **MCP layer (apps/beach/src/mcp/terminal/tools.rs + server.rs)**
   - New tool descriptors:  
     - `controller/acquire` – request a controller lease for a child session (wraps the existing `/controller/lease` HTTP call).  
     - `controller/release` – release the lease.  
     - `controller/queue-actions` – send one or more serialized `ActionCommand`s to the host. The host validates the lease and delegates to `ControllerActionContext`.
   - Extend `TerminalSession` to carry an `Arc<ControllerActionContext>` reference so the MCP tools can interact with the controller pipeline.
   - Update `McpServer`’s tool dispatcher to route the new methods and ensure they respect the session filter and lease permissions.

3. **IPC CLI (`beach action`)**
   - Add fast path: when `beach action` is run locally with `--session <id>`, it attempts to connect to the MCP socket at `${XDG_RUNTIME_DIR}/beach/mcp/<session>.sock` (or the explicit `--socket` flag).  
   - If the socket responds and advertises the `controller/queue-actions` tool, the CLI invokes it instead of calling Beach Manager.  
   - If the IPC connection fails or the tool isn’t available, the CLI falls back to the existing HTTP POST path.
   - The command surface should be as simple as today’s HTTP path: `beach action --session <childSessionId> --controller-token <token> --bytes '<json>'`. When run inside the host, we can default the session id (read from `$BEACH_SESSION_ID`) so the call is ergonomic.

4. **Agent integration**
   - Update the Pong agent (and future CLI agents) to prefer IPC: detect `$BEACH_MCP_SOCKET` or run `beach action --session …` so actions stay on-box. This keeps the agent language-agnostic.

### Detailed Design

#### Host wiring
1. **Extend `SessionHandle` / host bootstrap**  
   - When `ControllerActionContext` is created (`apps/beach/src/server/terminal/host.rs:181`), store `Arc<ControllerActionContext>` inside the `TerminalSession` that we register with the MCP registry (currently we only pass `session_id`, `sync`, `writer`, `process`).  
   - Introduce a new struct `ControllerBridge` with references to `ControllerActionContext`, `IdleSnapshotController`, `ManagerActionClient`, etc. It provides:
     ```rust
     async fn queue_actions(&self, child_session: &str, lease_token: &str, actions: Vec<ActionCommand>, trace_id: Option<String>) -> Result<(), BridgeError>
     ```
     This method calls the existing queue path: obtains/validates controller lease, uses `ManagerActionClient::queue_actions` via fast-path (through `ControllerActionContext`). Optionally returns structured errors for backpressure.
     ```
2. **MCP registry changes (`apps/beach/src/mcp/registry.rs`)**  
   - Extend `TerminalSession` to include an `Option<Arc<ControllerBridge>>`. Update `McpTerminalSession::new` call sites to pass the bridge.

3. **MCP tools**  
   - Define new request structs in `apps/beach/src/mcp/terminal/tools.rs`:
     ```rust
     pub struct ControllerQueueRequest { pub child_session_id: String, pub controller_token: Option<String>, pub actions: Vec<ActionCommand>, pub trace_id: Option<String> }
     ```
   - Add descriptors to `list_tools` (exposed only when `controller_bridge.is_some()` and server not read-only).
   - Implement handlers that:
     1. Resolve/validate `child_session_id` (ensure this host controls that session).
     2. If `controller_token` is missing, fetch it from cached leases (maybe already known from attach handshake).
     3. Call `ControllerBridge::queue_actions`.
     4. Return `{ "status": "ok", "transport": "fast_path" | "http", "queued": <count> }`.
   - Error handling: include queue depth info when throttled, `fast_path_not_ready`, etc.

4. **Lease coordination**  
   - Reuse `LeaseManager`: require clients to call `controller/acquire` before `queue-actions`. The acquire handler requests a lease via the manager HTTP API (existing code at `apps/beach/src/mcp/terminal/tools.rs::handle_acquire_lease` can be extended to support controller leases). Returned lease ID is stored in the MCP connection (per-session).  
   - `queue-actions` must verify the lease is still valid; if expired, reject with `invalid_lease`.

5. **Ergonomic CLI**  
   - Introduce a helper in `apps/beach/src/mcp/client.rs` (new module) that opens the MCP socket and calls tools programmatically.  
   - `beach action`:  
     1. If `--session` is provided, attempt to locate MCP socket `~/.beach/mcp/<session>.sock` (or `BEACH_MCP_SOCKET`).  
     2. Call `controller/queue-actions`.  
     3. Report success/fallback.  
     4. Provide `--no-ipc` flag to force HTTP for compatibility.  
     5. Document environment detection inside host sessions (expose `BEACH_SESSION_ID`, `BEACH_MCP_SOCKET`).

6. **Performance considerations**  
   - Keep the MCP tool simple: accept raw bytes, avoid extra serialization (use `serde_json::from_value` once).  
   - Reuse existing fast-path send logic; no extra copies on the hot path.

### Implementation Steps
1. **Host refactor**  
   - Update `ControllerActionContext` with a public async `dispatch_actions` method that accepts `Vec<ActionCommand>` and uses current logic. Return `ControllerDispatchOutcome { via_fast_path: bool }`.
   - Update `McpTerminalSession::new` to accept `Arc<ControllerActionContext>`.

2. **MCP registry / TerminalSurface**  
   - Add `controller_bridge: Option<Arc<ControllerBridge>>>` to `TerminalSession`.  
   - Thread this through `TerminalSurface` so tools know whether controller operations are available.

3. **Tool additions**  
   - In `apps/beach/src/mcp/terminal/tools.rs`:  
     - Define descriptors for the new controller tools (names, params schema).  
     - Implement request parsing + dispatch to `ControllerBridge`.  
     - Update `list_tools` to conditionally expose these tools.

4. **MCP server**  
   - Update `tools::handle_*` to route to the new controller handlers.  
   - Ensure `read_only` mode hides controller tools.

5. **CLI client**  
   - Add an MCP client helper (`apps/beach/src/mcp/client.rs`) that can call tools by name.  
   - Modify `beach action` command to attempt IPC first (unless `--no-ipc`). Provide `--session` and `--socket` overrides.

6. **Agent**  
   - Update Pong agent runner to prefer `beach action --session` when `$BEACH_MCP_SOCKET` is present.  
   - Keep HTTP path as fallback for older hosts.

7. **Testing**  
   - Unit tests for new MCP tool handlers (mock `ControllerBridge`).  
   - Integration test: spin up host, connect to MCP socket, send fake action, verify manager receives `controller.actions.fast_path`.

### Prompt for Implementation
```
You are Codex working in /Users/arellidow/development/beach. Implement the “Controller IPC Bridge Plan” in docs/private-beach/controller-ipc-bridge-plan.md. Key requirements:
- Extend the host MCP layer so a CLI process can call a new controller queue API over IPC.
- Expose MCP tool(s) that accept controller actions and use ControllerActionContext to dispatch them via fast-path/HTTP.
- Ensure lease validation + trace IDs still work.
- Update beach CLI (`beach action`) to prefer the local IPC tool when `--session` or BEACH_MCP_SOCKET is available, with HTTP fallback.
- Update the Pong agent runner to call the new IPC path when possible.
- Add tests/documentation as needed.
Follow the design doc closely and keep the new CLI experience fast and ergonomic.
```

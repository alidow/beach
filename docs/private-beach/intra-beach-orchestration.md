# Intra-Private-Beach Orchestration

## Goals
- Allow any session that belongs to a Private Beach to observe and coordinate other sessions through MCP APIs exposed by the Private Beach manager.
- Introduce a lightweight session harness that instruments terminals/GUI streams without requiring application awareness of Beach or Private Beach semantics.
- Maintain a low-latency, server-hosted cache of session state (terminal buffers, GUI frames, metadata) to enable instant cross-session visibility.
- Provide an action dispatch layer for keyboard, mouse, and byte-sequence injections so agents and humans can steer remote sessions programmatically.
- Showcase the capability with a flagship Pong demo that mixes TUI, Windows GUI (via Beach Cabana), and an MCP-driven “manager” agent.

## Capabilities Overview
- **Session Directory:** MCP tool returning metadata for active sessions (type, location, harness capabilities, status heartbeat).
- **State Snapshot:** MCP tool streaming or snapshotting current render state from the replicated cache; harnesses push diffs (terminal screen, GUI frame metadata) so applications remain unaware.
- **Action Dispatch:** Harness-level command queue that accepts manager-issued inputs (terminal bytes, mouse/keyboard events) and executes them locally; transport can be brokered (Redis/NATS) or direct (WebRTC) depending on latency profile.
- **Access Control:** Private Beach manager enforces that only sessions within the same private beach (or explicitly shared contexts) can query/act on each other.
- **Observability Hooks:** Every action and state request is auditable for future billing, debugging, and compliance tooling.

## State Cache Strategy
- **Terminal Sessions:** Harness emits run-length encoded VT grid diffs capped at 80×24 (configurable). Cache stores the last full frame plus rolling diff log (e.g., last 2 seconds) to reconstruct snapshots quickly. Estimated throughput ~2 MB/s for 100 sessions at 10Hz; sharding per private beach keeps hotspots isolated.
- **GUI Sessions:** Harness generates frame descriptors (ball/paddle bounding boxes, fps, hash) every 50–100 ms and only pushes raster frames on explicit request. Cache stores descriptors in Redis hashes and spills occasional thumbnails to object storage.
- **Retention & Eviction:** Each session namespace has configurable TTL (default 10 s) and memory ceiling; excess entries drop oldest diffs. Manager regenerates full snapshots when cache misses occur.
- **Scalability:** State cache is fronted by Redis Cluster for horizontal scaling; metrics monitor write amplification. Future optimisations (vector diff codecs, delta compression) plug into the harness without changing consumer APIs.

## Reference Architecture
1. **Session Harness (Beach Buggy)**
   - When a process joins a Private Beach, a thin wrapper spins up alongside it (Beach terminal shim or Cabana relay).
   - Harnesses (implemented via `crates/beach-buggy`) establish the MCP backchannel to the manager and declare capabilities (e.g., `supports_terminal_bytes`, `supports_gui_pointer`).
   - Application binaries stay “dumb”; all Beach awareness lives in the harness.
2. **State Replicator**
   - Receives incremental updates emitted by the harness (terminal diff, GUI frame hash, cursor state).
   - Normalizes and stores latest view in Redis (per-session cache key).
   - Supports change feeds so watchers can subscribe instead of polling.
3. **Command Transport**
   - Manager emits actions addressed to a harness; default path uses a shared message bus for durability and multi-controller arbitration.
   - Harnesses can optionally negotiate direct WebRTC data channels with managers for ultra-low-latency, high-frequency control loops.
   - Harness maintains a local FIFO so commands can queue even if the underlying app momentarily stalls; acknowledgements include timestamps for latency tracking.
4. **Policy & Mediation Layer**
   - Validates permissions, rate limits, and conflict resolution (e.g., multiple controllers).
   - Exposes admin override for human operator to pause automation or force control handoff.

## MCP Surface (Draft)
- `list_sessions`: Returns IDs, labels, media type (`terminal`, `cabana_gui`, `scoresheet`), current controller, and health metrics.
- `get_session_state`:
  - `mode`: `snapshot` (returns last known render) or `stream` (opens streaming diff channel).
  - `format`: `terminal_ansi`, `structured_grid`, `gui_frame_ref`.
- `queue_action`:
  - Supports `terminal_write`, `key_event`, `pointer_move`, `pointer_click`, `pointer_scroll`.
  - Options for priority, deduplication token, expiration, and transport hint (`brokered` vs `direct_webrtc`).
- `set_ball_state`/`custom_actions`: Placeholder for game-specific or domain-specific verbs surfaced by harness-level extensions when needed.

## Pong Showcase Flow
1. **Left Paddle (TUI)**
   - Plain terminal app runs with zero Beach awareness.
   - Terminal harness reports state diffs and accepts manager-issued `terminal_write` commands.
2. **Right Paddle (Windows GUI)**
   - Native Windows app streams through Beach Cabana.
   - Cabana harness abstracts GUI capture and input injection; manager sees normalized metadata (bounding boxes, frame hashes).
3. **Manager Agent**
   - Polls both sides using high-frequency `get_session_state` in streaming mode.
   - Calculates ball trajectory, emits paddle commands, and sets cross-session ball state when ownership changes.
4. **Scoreboard Session**
   - Simple TUI that updates via `terminal_write` triggered by the manager when a side scores; harness supplies diff stream for spectators.
5. **Spectator View**
   - Private Beach dashboard arranges all four sessions; observers can watch in real time without interfering.

## Controller Arbitration
- **Controller Lease:** Manager maintains a lease per session (`controller_id`, `expires_at`). Agents renew leases via `controller.renew` RPC; humans taking control request lease transfer, triggering a countdown overlay.
- **Mid-Action Takeover:** When a new controller wins the lease, harness flushes pending commands and ACKs them as `preempted`. Manager sends `controller_released` notifications so prior controllers can degrade gracefully.
- **Queue Semantics:** Actions are FIFO within a controller lease. Different controllers’ commands are rejected with `403 controller_mismatch`. Harness exposes queue depth; managers slow command rate when buffer grows beyond threshold.
- **Emergency Stop:** Admins can issue `controller.revoke` that pauses command execution until a new lease is granted, safeguarding against runaway agents.

## Open Questions
- How do we handle conflicting control when a human attempts to take over a session already driven by an agent?
- Should the state cache store raw frames or normalized vector representations to optimize network cost?
- What guarantees do we offer around action ordering, especially when multiple sessions queue actions against the same target?
- Do we need per-action confirmation hooks (ack/nack) for audit logs and UI display?
- How frequently can we poll/stream state before hitting performance ceilings on terminals and Cabana streams?
- What sandboxing is required so that an agent cannot exfiltrate or misuse another session’s credentials or file system?
- How do we package the harness so it attaches to arbitrary processes (containers, SSH sessions, Windows apps) without requiring app changes?
- What is the rollback story if controller lease transfer fails mid-flight (e.g., harness crash)?

## MCP Schema (v0)
- **Envelope:** All requests/responses use JSON-RPC 2.0 over MCP transport (`id`, `jsonrpc`, `method`, `params`). Errors follow MCP convention with `code` (int) and `message`.
- **Methods:**
  - `private_beach.list_sessions` → `[{id, label, media_type, harness_capabilities, controller_id, health}]`
  - `private_beach.subscribe_state` (`session_id`, `mode`, `format`, `since_seq?`) → stream of `{seq, timestamp, payload, checksum}`
  - `private_beach.queue_action` (`session_id`, `controller_token`, `actions:[{id, type, payload, priority?, dedupe_key?, expires_at?, transport_hint?}]`)
  - `private_beach.ack_actions` (`session_id`, `acks:[{id, status, applied_at, latency_ms?, error_code?, error_message?}]`)
  - `private_beach.acquire_controller` (`session_id`, `requestor_id`, `ttl_ms`, `reason?`) → `{controller_token, expires_at}`
  - `private_beach.release_controller` (`session_id`, `controller_token`) → `{released:true}`
- **Error Codes:**
  - `40001 invalid_capability`
  - `40002 controller_conflict`
  - `40003 action_expired`
  - `50001 transport_unavailable`
  - `50002 harness_unreachable`
- **Retry Semantics:** `400`-series are permanent; caller must adjust input. `500`-series invite exponential backoff. Action dedupe keys prevent duplicates if clients retry.
- **Checksums:** State payloads include `checksum` (xxHash64) so manager verifies integrity before writing to cache; mismatches trigger re-fetch.

## Next Steps
1. Design MCP schema messages for the three core tools and define authentication tokens per session harness.
2. Prototype the state replicator path using existing terminal diff events from `apps/beach`, encapsulated in the harness.
3. Define the command transport interface (brokered + optional WebRTC) and extend terminal/GUI harnesses with a lightweight consumer loop.
4. Build the Pong demo pipeline as an integration test harness to validate latency and conflict control scenarios.
5. Iterate on dashboards to visualize agent control, queue backlog, and session health indicators.

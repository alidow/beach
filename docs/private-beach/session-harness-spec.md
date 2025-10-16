# Private Beach Session Harness Specification

## Intent
- Wrap any Beach session (terminal, GUI, future media types) with a sidecar that provides Private Beach awareness without modifying the hosted application.
- Standardise state streaming, command intake, and telemetry so managers and agents interact with sessions through consistent MCP primitives.
- Enable flexible transport choices (brokered vs. peer-to-peer) while enforcing security boundaries and audit trails.

## Core Responsibilities
1. **Attachment & Identity**
   - Launch alongside a session when Beach establishes a connection (terminal attach, Cabana GUI stream, etc).
   - Authenticate with the Private Beach manager using scoped tokens derived from Beach Gate/OIDC identity plus session metadata.
   - Register capabilities (`terminal_diff_v1`, `gui_frame_meta_v1`, `keyboard_input`, `pointer_input`, etc) and receive a harness ID.
2. **State Capture**
   - Terminal harness: tail the PTY buffer, compute VT-grid diffs, normalise into compact MCP payloads, and push at configurable cadence (default ≤100 ms).
   - GUI harness (Cabana): tap existing capture pipeline, extract lossy-compressed frame metadata (bounding boxes, frame hashes) + low-FPS preview frames when requested.
   - All harnesses emit heartbeat + optional semantic hints (cursor pos, focused pane) without interpreting application-specific meaning.
3. **Command Execution**
   - Maintain local FIFO queue for incoming actions with priorities and expirations.
   - Apply validated commands to the underlying session (write bytes to PTY, synthesize pointer/keyboard events).
   - Send acknowledgements containing execution timestamp, status (`ok`, `rejected`, `expired`), and optional diagnostic info.
4. **Transport Management**
   - Default path: subscribe to Private Beach broker (Redis Streams/NATS) scoped to the private beach and session.
   - Optional fast path: negotiate WebRTC data channels with authorised managers/agents; fall back to broker if peer link fails.
   - Provide flow-control signals (queue depth, stall warnings) upstream so managers can adapt pacing.
5. **Security & Isolation**
   - Enforce per-command capability checks; reject actions beyond declared scope and require controller token tied to current lease.
   - Run harness under an unprivileged user/namespace; only PTY/GUI device handles are exposed. Optional seccomp/Mac sandbox profiles restrict filesystem and network access.
   - Config and signing keys pulled from Beach Gate–issued secrets manager; harness never persists credentials locally.
   - Log every state emission and command execution to Private Beach audit stream (with redactable payloads); sensitive data can be hashed before logging.
   - Escape hatch hooks (e.g., uploading files, clipboard access) require explicit opt-in capabilities and human confirmation.
6. **Lifecycle & Recovery**
   - Persist transient state (last diff hash, command cursor) so reconnects resume cleanly.
   - Advertise readiness states (`initialising`, `active`, `degraded`, `offline`) to the manager.
   - Surface local health metrics (CPU, latency) for observability.

## Protocol Surface (MCP Extensions)
- `session.register_capabilities`: invoked on attach; returns harness ID, broker topics, optional WebRTC offer parameters.
- `session.push_state`: harness → manager streaming channel, batched diffs tagged with sequence numbers.
- `session.pull_actions`: harness-initiated long-poll or streaming subscription; returns ordered actions.
- `session.ack_action`: harness reports result for each action ID.
- `session.signal_health`: periodic heartbeat with latency histogram, queue depth, custom warnings.

## Implementation Outline
- **Runtime:** Rust crate for terminal harness (leveraging existing Beach PTY plumbing), TypeScript/Node module for Cabana harness (integrated in web runtime), unified protocol module shared across clients.
- **Configuration:** JSON manifest per private beach defining harness policies (state cadence, allowed transports, encryption keys).
- **Extensibility:** Capability registry so new media types (screen share, audio) publish their own diff format without changing manager core.
- **Testing:** Harness simulator feeding synthetic terminal/GUI streams; chaos suite introducing latency spikes, command floods, and reconnect scenarios.

## Failure Handling
- Broker outage: harness buffers commands locally, switches to peer-to-peer if configured, and raises degradation signal.
- Peer congestion: harness back-pressures managers via `queue_depth` metric; manager must slow command rate.
- Session crash: harness emits `offline` event; manager redistributes control or pauses automation; upon restart, harness performs state resync handshake.

## Controller Handshake Flow
- On startup, harness has no active controller. Managers or humans call `acquire_controller`; harness validates token with Beach Gate, caches lease metadata, and emits `controller_changed` event.
- Incoming commands must carry the lease-bound `controller_token`; mismatches are rejected with `controller_conflict`.
- When lease expires or a new controller is granted, harness flushes pending actions, marks them `preempted`, and transitions to the new token.
- Harness exposes API to pause command execution (`controller.suspend`) used during human takeover countdowns; resumed via `controller.resume`.

## Repository Layout Proposal
```
apps/
├─ beach/                # existing open-source terminal foundation
├─ beach-human/          # desktop client
├─ beach-cabana/         # GUI streaming surface (with harness module)
├─ private-beach/        # New Next.js frontend + API for Private Beach manager UI
└─ beach-manager/        # Manager/control-plane service (Rust or TypeScript)

crates/ or packages/
├─ session-harness/      # Shared harness runtime (Rust core)
├─ harness-proto/        # MCP schema + generated bindings
├─ cabana-harness/       # JS/TS wrapper around Cabana capture
└─ manager-sdk/          # Client library for managers/agents to consume harness APIs

docs/
└─ private-beach/
   ├─ vision.md
   ├─ intra-beach-orchestration.md
   ├─ pong-demo.md
   └─ session-harness-spec.md   # this document

infrastructure/
├─ terraform/             # environment provisioning
└─ k8s/                   # manifests for manager + broker + redis
```
- `apps/private-beach` houses the premium web experience (Next.js, Tailwind, shadcn).  
- `apps/beach-manager` exposes APIs, orchestrates harness communication, and manages broker/WebRTC negotiation.  
- Harness code lives in shared crates/packages so both Beach core and Private Beach can reuse the same sidecar logic.

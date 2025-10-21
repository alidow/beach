# Beach Buggy Harness Specification (Refocused)

## Intent
- Make the harness an **optional enrichment layer** that plugs into any Beach session without touching the host binary or primary transport.
- Let authorised clients ask for **derived views** of the session (semantic text, motion vectors, high-level events) when raw terminal/GUI diffs are too heavy or low-level.
- Keep the core `apps/beach` runtime responsible for connectivity, contention management, and canonical diff streaming; the harness only listens and transforms.

## What the Harness *Does*
1. **Attachment on Demand**
   - Spins up only when a consumer (Manager, controller, analytics job) requests a declared capability.
   - Authenticates with scoped tokens, advertises optional modules (`terminal_semantic_v1`, `gui_motion_vectors_v1`, `cursor_intent_v1`), and receives a harness ID for telemetry.
2. **Derived State Transforms**
   - Subscribes to the existing WebRTC/TURN/WSS data channel that the host provides.
   - Produces alternate representations: VT grid → text/layout blocks, Cabana frames → object detections, “interesting deltas only” streams, etc.
   - Supports per-subscriber cadence/back-pressure so expensive transforms can be throttled or paused.
3. **Contextual Input Metadata (Optional)**
   - Observes acknowledgements emitted by the core host and replays metadata (which controller acted, how long it took, conflict reason) as structured events.
   - **Does not** arbitrate or queue inputs; if two controllers clash, the host’s built-in contention rules still apply.
4. **Transport Alignment**
   - Reuses the host’s peer connection and simply adds new RTCDataChannels (e.g., `mgr-semantic-state`, `mgr-vision-events`, `mgr-input-meta`).
   - If the host falls back to TURN/WSS, the harness rides along; if peer transport is unavailable and no entitlement exists, transforms are suspended.
5. **Security & Isolation**
   - Runs under an unprivileged context with read-only access to host diffs.
   - Enforces capability-level authorisation; unentitled consumers are rejected.
   - Emits lightweight audit logs (consumer ID, transform type, latency) without duplicating raw payloads.
6. **Lifecycle**
   - Maintains small checkpoints to resume transforms after reconnects.
   - Publishes availability (`inactive`, `warming`, `active`, `degraded`) so clients can adjust expectations.

## What the Harness *Does Not* Do
- It does **not** replace the host for base diff streaming—`apps/beach` remains authoritative.
- It does **not** broker controller contention or maintain its own command queue.
- It does **not** fall back to Redis/HTTP streaming; peer-to-peer is the golden path, with TURN/WSS only when the host already authorised it.

## Protocol Surface (MCP Extensions)
- `session.register_capabilities` – harness advertises optional transforms; manager returns harness ID and approved modules.
- `session.request_transform` – client asks the harness to start/stop a transform with parameters (bounding boxes only, cadence, filters).
- `session.push_transform` – harness → client payloads with sequence numbers and provenance.
- `session.describe_input` – optional metadata stream summarising host-acknowledged actions (purely informative).
- `session.signal_health` – heartbeat covering transform latency, backlog, and resource usage.

### RTC Channel Examples
| Label               | Direction        | Reliability | Payload                                     |
|---------------------|------------------|-------------|---------------------------------------------|
| `mgr-semantic-state`| Harness → client | Unordered   | JSON layout/semantic summaries              |
| `mgr-vision-events` | Harness → client | Unordered   | Motion vectors, detected objects            |
| `mgr-input-meta`    | Harness → client | Ordered     | Metadata about actions the host applied     |

ICE/SDP reuse the host’s negotiation; no extra HTTP handshake is needed.

## Implementation Outline
- **Core crate:** `crates/beach-buggy` exposes transform primitives, capability registry, and channel wiring helpers.
- **Adapters:** thin modules per media type (terminal, Cabana GUI, future audio/video) that plug into the core crate.
- **Manager integration:** manager requests transforms only when a viewer/controller subscribes, keeping idle sessions lightweight.
- **Testing:** synthetic streams feed the harness to ensure transforms are accurate, rate limited, and resume after reconnects.

## Usage Examples
- **Beach Cabana analytics:** publish bounding boxes for the ball/paddle so an agent reacts without downloading full frames.
- **Terminal summarisation:** emit rolling text paragraphs or structured JSON from VT diffs for LLM agents.
- **Audit overlay:** stream input metadata so auditors know which controller triggered each action without inspecting raw bytes.

## Open Questions
- Do we need persistent transform caches for replay/export, or is on-demand enough?
- How do we price/entitle expensive transforms (OCR, vision) across public vs. private sessions?
- Should transforms be daisy-chained (e.g., harness A feeds harness B), or do we keep a single sidecar per session?

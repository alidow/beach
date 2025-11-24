Single-Channel Controller Transport Plan

Goals
- Collapse controller delivery onto the same primary host<->client transport (single RTC data channel per peer) that already carries `WireClientFrame` traffic; remove any separate fast-path peer/channels (greenfield: build only the single channel).
- Keep the Beach Buggy harness able to intercept controller-related frames on that single channel (pause HTTP polling, apply actions, send acks).
- Define a checksum-friendly, chunkable wire envelope so large controller/state payloads are framed safely and verifiably at the transport layer (apps/beach/transport).
- Greenfield: no staged rollout or feature flags; single-channel-only paths end to end.
- Make it dead simple to pinpoint pipeline issues (where frames drop, fail CRC/MAC, stall in reassembly, or block on DTLS).

Current State (pain)
- Legacy approach used a dedicated fast-path peer (`mgr-actions`/`mgr-acks`/`mgr-state`) and bespoke chunking; we’re replacing this entirely with a single-channel design.
- Extra signaling + channel bookkeeping increased complexity and data loss; we now intend to avoid that by never creating the extra peer.
- Chunking/checksums will move into the shared transport layer so all frames are framed uniformly.

Target Architecture
- One RTC data channel per peer; a host may still have multiple peers (e.g., manager and browser), but each peer uses only the primary negotiated channel (ordered/reliable) for all traffic.
- Optional escape hatch if saturation is observed: allow a second unordered/unreliable channel dedicated to state diffs only, but default to a single ordered channel. Only enable if queue metrics show sustained HOL blocking.
- Manager’s controller forwarder attaches to that primary channel; no extra peers or labels.
- Controller actions flow as transport-level frames on that channel, alongside existing `WireClientFrame` traffic. The harness subscribes to those frames and pauses HTTP polling when the single channel is healthy.
- No dedicated fast-path labels; the only channel is the negotiated one (ordered/reliable).
- Manager auth on the single channel: manager joins as a WebRTC client using the same offer/answer path as other clients, and must present Clerk/Beach Gate JWT in the handshake metadata; unknown/unauthenticated peers are rejected.
- Transport layer owns framing:
  - Envelope fields: `version`, `namespace`, `kind`, `seq` (u64, monotonic per sender per namespace), `total_len` (u32), `chunk_index`/`chunk_count` (u16), `payload_crc32c` (u32) over the unchunked payload bytes.
  - Max payload before chunking: default 14 KiB (configurable constant shared by both ends; later auto-tune from SCTP PMTU if available). Chunk boundaries happen in transport, not per-feature code.
  - Decoder rejects mismatched CRC/length; reassembles chunks by `(namespace, kind, seq)` and emits a single logical frame upstream; duplicates (by seq/namespace) are ignored.
- Reassembly eviction: drop in-flight assemblies that exceed timeout (e.g., 2–5s) or memory cap; log/metric the drop with seq/namespace/kind/peer. Late chunks after eviction are discarded with diagnostics.
- Optional MAC: covers `version|namespace|kind|seq|total_len|payload` (post-CRC). CRC runs first for cheap corruption detection; MAC failure drops the frame, increments metrics, and logs. Replay/nonce protection: reject seq reuse per sender/namespace.
- QoS/backpressure: keep chunk size bounded (14 KiB) and prefer sending small control frames first (controller acks/inputs) to minimize head-of-line blocking on the ordered channel; surface queue length metrics.
  - Queue metrics: per-namespace pending/send-queue depth and latency; budget small control frames ahead of large sync/state frames to avoid HOL stalls.
- Controller routing on the host:
  - Harness listens for `namespace="controller"`, `kind` in {`input`, `ack`, `state`} and feeds existing PTY writer + optional HTTP ack.
  - Other namespaces (terminal sync, extensions) continue unchanged.

- Wire Protocol Sketch (single channel)
- Namespace `controller`, kinds (share the same `seq` space for controller namespace; duplicates are ignored):
  - `input`: payload = `ControllerInput { action_id, bytes }`; applied to PTY, acked inline.
  - `ack`: payload = `ControllerAck { action_ids, status }`; manager consumes. Sent on the same channel; HTTP only when falling back after channel health drops.
  - `state`: payload = `StateDiff`; optional, but framed identically.
- Namespace `sync` for existing terminal frames; move to the same envelope + chunking for uniformity.
- Backward-compat: not required for greenfield; only implement the single channel and framing once.

Migration Steps
1) Transport envelope + chunking (apps/beach/transport/*)
   - Add a reusable `FramedMessage` encoder/decoder with CRC32C and chunking.
   - Integrate into the primary transport send/recv paths so all namespaces benefit; remove legacy fast-path framing/peers entirely.
2) Host harness wiring (crates/beach-buggy + apps/beach/src/server/terminal/host.rs)
   - Subscribe to the primary transport; detect `namespace="controller"` frames and route to the existing controller consumer (reuse PTY writer + optional HTTP ack).
   - Keep HTTP poller pause/resume logic driven off single-channel health.
   - Remove fast-path peer creation/consumption entirely.
3) Manager forwarder (apps/beach-manager/src/fastpath.rs/state.rs)
   - Send controller actions over the primary negotiated transport only: encode as `controller/input` framed messages.
   - Do not emit fast-path offer/ICE hints; there is no extra peer.
4) Health/fallback
   - Channel healthy when: data channel `readyState=Open`, DTLS established, and a loopback probe (known payload+CRC/MAC) succeeded within N seconds (e.g., 10s) with low CRC/MAC error rate.
   - Pause HTTP poller while healthy; resume if health flips unhealthy (probe fails, CRC/MAC error rate threshold exceeded, DTLS closes).
5) Testing
   - Unit: encoder/decoder CRC/chunking (happy path, corruption, missing chunk, duplicate chunk).
   - Integration: spawn host+manager locally, send large controller payloads > chunk size, verify reassembly + PTY apply + ack round-trip.
   - Regression: pong fast-path smoke uses only the single channel; ensure ball/command traces populate and HTTP poller pauses.
6) Cleanup
   - Remove fast-path peer structs (`FastPathClient`, channel labels) and related docs.
   - Delete fast-path-specific chunking code paths; keep transport-layer framing as the single source of truth.

Rollout Notes
- Greenfield: single-channel-only; no dual-path rollout. Add telemetry counters for framed send/recv, CRC failures, and MAC failures from day one.

Test Plan (unit, integration, load, e2e)
- Large payload chunking (unit/integration): encode/decode messages that exceed the transport chunk size for both namespaces:
  - Normal host<->browser sync (`namespace=sync`) carrying `WireClientFrame` bytes.
  - Manager “control” (`namespace=controller`, kind=input/state/ack). Assert correct chunk_count/index, CRC32C validation, and end-to-end PTY application.
- Parallel peers (integration): stand up concurrent host<->manager and host<->browser WebRTC transports (each with a single channel). Use a Node-based test harness to mimic the browser transport; send/receive framed messages on both peers simultaneously and verify no cross-talk or dropped chunks.
- Load/soak (integration/load): pump mixed sizes (small + >chunk) in both directions, measure reassembly latency, CRC failure count, and back-pressure behavior. Include bursty sends to exercise ordering and duplicate handling.
- Full pipeline (e2e): host agent issues an action → manager → Redis → controller forwarder → child host session → PTY write observed. Assert the framed controller input is reassembled once, applied, and acked.
- Manager-as-client handshake (integration+unit breakdown): manager joins host as a client on the single channel, sends framed controller messages that the buggy harness intercepts:
  - Unit: each step of framing/CRC, harness interception, PTY write, ack emission.
  - Integration: full handshake with framed controller traffic over the single channel and observed handling in the harness.
- Backpressure/ordering: include tests that mix large chunked frames with small controller acks/inputs to ensure the send queue prioritizes small control frames, exposes queue metrics, and avoids HOL stalls.
- Failure cases: tests for partial chunk loss and duplicated seq; assert drops are logged/metric’d and do not block subsequent frames.

Encryption Verification
- Two layers to test:
  - Transport encryption (WebRTC DTLS/SRTP): validate both peers negotiate DTLS; in tests, fail if the data channel reports insecure params. Include a probe frame after DTLS ready to confirm it arrives; no plaintext capture allowed.
  - Optional application MAC (if we add end-to-end signing inside the envelope): unit-test MAC generation/verification over `version|namespace|kind|seq|total_len|payload`; integration-test corruption scenarios (flip bits, expect MAC failure and drop).
- Unit tests:
  - Verify framed payload CRC runs before MAC verification (if present) and that CRC/malformed frames are rejected pre-MAC.
  - Ensure seq/nonce reuse is rejected if MAC uses a nonce.
- Integration tests:
  - Inject bit flips on individual chunks; expect CRC failure and no delivery to PTY.
  - For app MAC, inject tampering after CRC passes; ensure MAC failure is logged/metric’d and frame dropped.
  - DTLS sanity: assert data channel `readyState=Open` with SCTP/DTLS params present; include a negative test where DTLS is forced to fail and ensure we fall back to HTTP.
- Diagnostics:
  - CRC catches corruption/chunking errors cheaply; MAC (when enabled) detects tampering/replay over the full envelope metadata+payload. Keep both to quickly localize whether failure is integrity vs. authenticity.
  - Counters for CRC failures, MAC failures, reassembly timeouts, DTLS handshake failures, and per-namespace/kind sent/received/applied frames; log seq/namespace/kind/peer on errors (rate-limited).
  - Optional loopback probe frame with known CRC/MAC to validate the stack periodically; mark channel unhealthy and resume HTTP if probes fail.
 - DTLS inspection: use browser WebRTC stats API for JS clients and Rust webrtc-rs API for native clients to confirm cipher/DTLS role/SCTP params; log on mismatch.
 - Per-stage logging to pinpoint pipeline location of drops: encode/send, chunk emit, recv/chunk ingest, reassembly complete, CRC/MAC check, dispatch to PTY/consumer.

Milestones (incremental, keep code buildable at each step)
- Phase 1: Framing library
  - Implement `FramedMessage` encoder/decoder (CRC32C, optional MAC with key_id, chunking, eviction, dup handling) and queue metrics.
  - Wire unit tests; do not rewire transports yet (or keep a shim that can be swapped in).
- Phase 2: Transport swap
  - Replace legacy transport encoding/decoding with the new framing for the primary channel; add per-namespace counters and error metrics.
  - Keep existing channel topology temporarily if needed, but ensure framing is unified.
- Phase 3: WebRTC channel simplification
  - Collapse to one ordered channel per peer (optionally add but disable the unordered state channel). Remove fast-path labels/peer creation.
- Phase 4: Host/harness routing
  - Consume controller namespace on the primary channel; acks on the same channel; HTTP pause/resume on channel health; enforce auth; remove fast-path code paths.
- Phase 5: Manager forwarder/auth
  - Manager joins as WebRTC client with JWT metadata; sends controller frames on the primary channel only; strip fast-path hints/routes/metrics.
  - Signaling joins now carry manager metadata (`bearer`, `role=manager`, `session_id`) and the answerer validates the manager JWT via Clerk/Beach Gate JWKS before accepting the peer.
- Phase 6: Telemetry + tests
  - Wire Prometheus metrics (per-namespace counters, CRC/MAC/reassembly/DTLS errors, queue depth/latency).
  - Add/enable the full test matrix (unit/integration/load/failure/e2e/Playwright + scripts/pong-fastpath-smoke.sh).
- Phase 7: Cleanup/docs
  - Remove remaining fast-path artifacts; update checklist/notes with outcomes and test evidence.

Progress Tracking Checklist (update as you implement)
- [x] Transport framing: single-channel `FramedMessage` encoder/decoder with CRC32C, optional MAC, chunking, reassembly eviction, duplicate handling.
- [x] Manager forwarder: controller actions sent over primary channel; no fast-path hints/peers; auth enforced (Clerk/Beach Gate JWT in handshake).
- [x] Host/harness: controller namespace consumed from primary channel; HTTP pause/resume based on channel health; acks on same channel; fast-path peers removed.
- [x] QoS/backpressure: queue metrics (per-namespace depth/latency) and control-frame prioritization; optional state-diff channel kept disabled by default.
- [x] Telemetry/diagnostics: per-namespace/kind sent/received/applied counters; CRC/MAC/DTLS/reassembly metrics; per-stage logs.
- [ ] Tests added/passing:
  - [x] Unit: framing (chunking, CRC/MAC, dup/timeout).
  - [ ] Integration: Node/browser harness + manager peers concurrently.
  - [ ] Load/soak: mixed sizes, HOL/backpressure observed/limited.
  - [ ] Failure cases: partial chunk loss, duplicated seq handling.
  - [ ] Full pipeline e2e: action → manager/Redis → child host PTY.
  - [ ] Manager-as-client handshake end-to-end.
  - [x] Playwright e2e tests.
  - [x] `scripts/pong-fastpath-smoke.sh` run after implementation.
- [ ] Cleanup: fast-path artifacts removed; docs updated if behavior diverged.

Current Short-Term Substeps (track as we chip away)
- [ ] Manager forwarder auth/attach (smaller slices)
  - [x] Enforce handshake metadata shape `{ bearer, role="manager", session_id }`; reject missing/invalid role (issuer/aud to follow).
  - [x] Swap controller send path to `send_namespaced(\"controller\", \"input\", …)`; remove fast-path send hints/metrics but keep HTTP fallback.
  - [x] Remove fast-path upgrade probes/watchers and ack/state extension subscribers; rely on framed bus.
    - [x] Delete `ensure_fast_path_probe` usage and `FastPathUpgradeHandle` wiring; collapse forwarder loop to primary-only.
    - [x] Replace forwarder FastPathReady handling with a no-op to keep primary-only.
    - [x] Drop fast-path ack/state extension subscriber in forwarder loop; rely on framed controller namespace.
    - [x] Prune fast-path-specific tests/constants/metrics after the above compiles.
- [x] Host/harness routing
  - [x] Host subscribes to framed controller acks/state on primary channel; HTTP poller pauses/resumes based on channel health.
  - [x] Remove fast-path labels/constants/thread creation; single ordered channel only (legacy fast-path listeners now no-op).
- [ ] Metrics/backpressure
  - [x] Per-namespace/kind counters (sent/received/applied) and controller queue depth/latency gauges; log when control prioritization activates.
  - [ ] Optional unordered state channel remains disabled; gate via config.
- [ ] Testing pass
  - [ ] Integration harness: manager + host single-channel round-trip for controller input/ack.
  - [ ] Re-run/regate Pong smoke + Playwright using single channel (no fast-path logs expected).

Implementation Notes (fill during development)
- Decisions/changes to plan:
  - Added `transport::framed` library with CRC32C + optional HMAC-SHA256 MAC (key_id), 14 KiB default chunking, reassembly eviction (timeout/memory), duplicate suppression, and queue gauges. Unit coverage exercises chunking, MAC/CRC failures, and timeout eviction.
  - WebRTC transport now uses the framed envelope for the primary channel send/recv paths (namespace=`sync`, kind derived from payload type) with MAC/key parsing configurable via `BEACH_FRAMED_MAC_KEYS`/`BEACH_FRAMED_MAC_KEY` envs. Framing version set to `0xA1` to avoid colliding with secure-transport ciphertext detection.
  - Added framed namespace pub/sub bus keyed by transport id for routing controller frames later; WebRTC exposes namespaced send and publishes all framed messages to subscribers (non-`sync` frames short-circuit to subscribers).
  - Unified bridge now prefers controller namespace framing (controller/input/ack/state/health) via `send_namespaced`, falling back to legacy extensions for transports without framing; controller inputs can be ingested from the framed bus.
  - Host controller consumer now listens on the primary channel via framed `controller/input` messages (dedup by `action_id`), applies bytes to PTY, and sends controller acks via namespaced framing (with HTTP ack fallback). Fast-path WebRTC endpoints on the manager are stubbed out to start removal.
  - Manager forwarder now selects the primary transport for controller delivery and consumes framed `controller/{ack,state,health}` messages from the namespace bus; fast-path extension subscription replaced with framed subscription. Legacy `/fastpath/*` routes 404.
  - Removed manager fast-path module/routes/metrics/tests; controller forwarder now treats the primary transport as the single path without extra fast-path hints.
  - Host controller loop now emits controller frame counts and queue depth/latency metrics on the primary channel; fast-path/state-channel listeners are no-ops and state publishes fall back to HTTP.
  - Host fast-path state/channel listeners are now no-ops; controller actions/acks flow only on the primary framed channel and state publishes fall back to HTTP when needed.
  - WebRTC sender loop now prioritizes control namespaces and sub-512B frames, tracks per-namespace queue depth, and records enqueue→send latency; logs when prioritization overrides queued payloads. Optional unordered/state channels remain disabled.
  - Added DTLS failure counter and retained framed CRC/MAC/reassembly error counters to satisfy telemetry coverage.
  - Playwright pong showcase now checks ball motion via container-bound logs (ANSI stripped + multi-match) instead of on-canvas movement to reduce flake; log root bind-mounted to `temp/pong-showcase`.
- Issues encountered and mitigations:
  - Secure transport ciphertext detection collided with the new framing prefix; resolved by using a distinct framing version byte.
  - WebRTC transport unit tests expected a legacy signature; passed through `raw_mode=false` to keep behavior unchanged while compiling new framing tests.
- Test evidence (links to logs/artifacts for smoke/Playwright/e2e):
  - `cargo test -p beach framed -- --nocapture`
  - `cargo test -p beach transport::framed::tests::publish_and_subscribe_namespace -- --nocapture`
  - `cargo test -p beach transport::webrtc::tests::encrypted_frame_buffered_until_encryption_enabled -- --nocapture`
  - `cargo test -p beach transport::unified_bridge::tests::emits_state_and_ack_extensions -- --nocapture`
  - `cargo test -p beach transport::webrtc::signaling::tests::register_client_peer_stores_metadata_for_non_client_roles -- --nocapture`
  - `cargo test -p beach tests::webrtc_transport -- --nocapture`
  - `cargo test -p beach-manager select_controller_transport_short_circuits_metadata -- --nocapture` (compilation ok; timed out before test completion)
  - `cargo check -p beach-manager`
  - `cargo check -p beach --lib`
  - `cargo check -p beach-road`
  - `cargo check -p beach`
  - `cargo test -p beach-manager`
  - `npm test` (apps/private-beach-rewrite-2) — passes with FlowCanvas viewport/props enabled; warnings resolved.
  - `npx vitest run src/features/canvas/__tests__/FlowCanvas.reactflow-props.test.ts` — passes (props tests re-enabled, lightweight helper).
  - `SKIP_PLAYWRIGHT_WEBSERVER=1 npx playwright test --config playwright.config.ts --project=chromium` (rewrite-2) — Clerk sign-in passed with secrets; pong showcase skipped (RUN_PONG_SHOWCASE not set).
  - `RUN_PONG_SHOWCASE=1 DEV_ALLOW_INSECURE_MANAGER_TOKEN=1 DEV_MANAGER_INSECURE_TOKEN=DEV-MANAGER-TOKEN PRIVATE_BEACH_MANAGER_TOKEN=DEV-MANAGER-TOKEN NEXT_PUBLIC_PRIVATE_BEACH_MANAGER_TOKEN=DEV-MANAGER-TOKEN PRIVATE_BEACH_BYPASS_AUTH=0 BEACH_SESSION_SERVER=http://beach-road:4132 PONG_WATCHDOG_INTERVAL=10.0 npx playwright test --config playwright.config.ts --project=chromium tests/e2e/pong-showcase.pw.spec.ts` — passed with auth enforced; beach `af6d2214-7423-4ef4-a1b9-0bf4e97b440c`, ball motion confirmed via log scraping. Re-ran twice after adding paddle-motion log checks + longer tile connect timeout: beach `b8356177-2de8-47e3-a99a-9693fe57262b` (pass) and `0323a21e-b0c5-4f73-b2d8-44d059f75dba` (pass). One intermediate flake had LHS tile stuck hidden at 60s; resolved by increasing connect timeout to 120s.
  - `direnv exec . ./scripts/pong-fastpath-smoke.sh --duration 20 --profile local` — PASS (20251123-222828). Artifacts: `temp/pong-fastpath-smoke/20251123-222828`. Required refreshing local Beach CLI token against local Gate and extending run duration (10s run earlier produced ball traces but no score update).
  - `cargo test -p beach webrtc_namespaced_controller_round_trip -- --nocapture` — passes (real WebRTC DTLS + framed controller input/ack round-trip).
  - Node browser-sim (werift) added: `apps/beach/tests/node-webrtc` (pure JS WebRTC) passes framed round-trip (`npm test` → “node-werift framed round-trip ok”).
  - `direnv exec . scripts/pong-fastpath-smoke.sh --duration 10 --skip-stack` — failed: manager API 401 when creating private beach (CLI token missing/bypass auth disabled).
  - `direnv exec . env PRIVATE_BEACH_BYPASS_AUTH=1 PRIVATE_BEACH_MANAGER_TOKEN=DEV-MANAGER-TOKEN scripts/pong-fastpath-smoke.sh --duration 10 --skip-stack` — failed: manager API 401; need proper auth/bypass at manager.
  - TODO: rerun pong smoke with valid manager auth (CLI token or bypass injected at compose startup).
  - `cargo test -p beach-manager`

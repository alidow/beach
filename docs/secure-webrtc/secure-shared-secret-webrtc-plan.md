# Secure WebRTC Shared-Secret Plan

## Background
- The Beach host and participant exchange WebRTC offers and answers through `beach-road`. Payloads are stored in Redis and fetched in plaintext.
- Session passphrases are only used to gate access; the server receives the passphrase during `POST /sessions/{id}/join`, and the SDP remains readable in transit and at rest.
- Once a peer connection is established, application data is sent over a single data channel without any additional end-to-end cryptography.
- A compromised signaling server (or someone with Redis access) can observe SDP payloads, tamper with ICE candidates, and read all post-handshake messages.

## Goals
1. Keep the session passphrase and all SDP payloads confidential from `beach-road`.
2. Provide integrity for signaling artifacts so tampering is detected before the WebRTC connection forms.
3. Derive long-lived send/receive keys shared only by the peers and re-wrap every Beach transport frame so post-connect traffic is opaque to the signaling tier.
4. Maintain compatibility with existing clients during rollout via feature flags and version negotiation.

## Document Map
- `secure-shared-secret-webrtc-plan.md` (this file): high-level goals, status, and next steps.
- `secure-webrtc/dual-channel-implementation-plan.md`: original app-layer handshake plan; now historical context.
- `secure-webrtc/dual-channel-webrtc-implementation.md`: notes from the initial implementation spike.
- `secure-webrtc/dual-channel-webrtc-spec.md`: protocol baseline for dual-channel/Noise work.
- `secure-webrtc/webrtc-dual-channel-status-and-plan.md`: status journal from the earlier rollout.
- `secure-webrtc/zero-trust-webrtc-spec.md`: long-form design for zero-trust signaling/transport.
- `secure-webrtc/beach-webrtc-handshake.md`: troubleshooting log for signaling/data-channel flows.

## Status Overview
- [x] **Phase 1 – Sealed Signaling (2025-02-15):** CLI offer/answer/ICE sealing landed (originally hidden behind `BEACH_SECURE_SIGNALING` during rollout; now enabled by default).
- [x] **Phase 2 – Noise Key Confirmation (2025-02-15):** Post-connect Noise XXpsk2 handshake over a dedicated data channel derives send/recv keys and a verification string (initially behind `BEACH_SECURE_TRANSPORT`; now always on).
- [x] **Phase 3 – Encrypted Transport Wrapper (2025-02-15):** CLI transports now wrap every frame with ChaCha20-Poly1305 using handshake-derived keys and per-direction nonces; replayed or tampered frames are rejected.
- [ ] **Phase 4 – Web Client Parity & Rollout:** Ship sealed signaling + transport encryption in beach-web, remove plaintext fallback after adoption (pending).

## High-Level Approach
1. **Shared-secret derivation**
   - Host and participant treat the passphrase (or join code) as low entropy input.
   - Use Argon2id with session-specific salt (`session_id || "beach-salt"`), tuned for interactive latency, to stretch into a 256-bit pre-shared key.
   - All subsequent key schedules HKDF this stretched key with explicit context labels and nonces to avoid material reuse.
2. **Sealed signaling channel**
   - Encrypt SDP offers, answers, and ICE candidates client-side before posting to `beach-road`.
   - Proposed framing: `version || handshake_id || nonce || ciphertext || auth_tag` using AES-256-GCM or ChaCha20-Poly1305 (via `ring` or `aes-gcm` crate).
   - `beach-road` continues to act as a dumb relay; it stores opaque blobs and never sees plaintext or passphrases.
   - Implement opportunistic fallback (plaintext) controlled by a CLI flag/env var during rollout.
3. **Post-connect Noise handshake**
   - Once the data channel opens, both sides run a Noise `XXpsk2` handshake (via `snow`) over a dedicated control channel.
   - Mix the Argon2id-derived key as PSK input and a DTLS exporter (when exposed) or SDP fingerprint to bind the channel to the negotiated connection.
   - Derive distinct send/receive keys (`K_send`, `K_recv`) via Noise’s built-in key schedule.
4. **Encrypted transport wrapper**
   - Introduce `EncryptedTransport` in `apps/beach/src/transport` that wraps the raw data channel.
   - Frame messages as `nonce || ciphertext || tag`, incrementing a 96-bit counter per direction and rekeying via HKDF every N messages or M minutes.
   - Update `TransportMessage` encode/decode paths to operate on plaintext while the wrapper handles AEAD.
5. **Verification and UX**
   - Add an out-of-band verification string (short authentication string) displayed to both peers after the Noise handshake succeeds.
   - Provide clear CLI / web hints if secure mode is disabled or if verification fails.

## Detailed Implementation Tasks
### 1. Cryptographic primitives
- Add `argon2`, `hkdf`, and `aes-gcm` (or `chacha20poly1305`) crates to `apps/beach` and audit dependencies.
- Introduce `crates/beach-crypto` helper with tested key-derivation and AEAD utilities shared between host/client code paths.

### 2. Client signaling changes (`apps/beach/src/transport/webrtc`)
- Extend `WebRTCSignal` payload to carry `ciphertext` + `encryption_version`.
- Before sending offers/answers/ICE, derive per-handshake sealing key: `HKDF(psk, "beach/sdp", handshake_id)`.
- On reception, attempt decrypt; if version unsupported, fall back (for older sessions) and surface telemetry.
- Update `SignalingClient` to avoid sending passphrase in the join message once PAKE lands (temporarily keep for backward compatibility with a hard deprecation date).

### 3. Server relay adjustments (`apps/beach-road`)
- Treat signaling payloads as opaque bytes (store base64 or binary).
- Skip passphrase verification once sealed signaling is mandatory; until then, retain verification but mark the field deprecated.
- Ensure Redis TTL refresh logic and queue semantics remain unchanged.

### 4. Noise handshake channel
- Create a reserved data channel label (`beach-secure-handshake`).
- Implement responder/initiator state machines using `snow` and integrate with the transport supervisor.
- Persist derived keys in `EncryptedTransport` and confirm both peers reach the same session id + verification string.

### 5. Transport encryption wrapper
- Wrap existing `Transport` trait objects with `EncryptedTransport` once the Noise handshake completes.
- Integrate nonce management and periodic rekey (call Noise `rekey()` or HKDF with message counter).
- Ensure control plane traffic (heartbeats, ready ACKs) routes through the encrypted channel after activation.

### 6. Testing & validation
- Unit tests for Argon2id/HKDF determinism and AEAD decrypt failure cases.
- Integration test that spins up two clients with a fake signaling server capturing stored blobs; assert ciphertext is unreadable and tampering breaks decryption.
- Integration test ensuring mismatched passphrases cause handshake failure before user data flows.
- Fuzz test the ciphertext parser to guard against panic on malformed data.

### 7. Rollout plan
- Ship with secure signaling + transport enabled by default (legacy flags removed; use the explicit `BEACH_INSECURE_SIGNALING=I_KNOW_THIS_IS_UNSAFE` / `BEACH_INSECURE_TRANSPORT=I_KNOW_THIS_IS_UNSAFE` / `VITE_ALLOW_PLAINTEXT=I_KNOW_THIS_IS_UNSAFE` escape hatches only for debugging).
- Enable in nightly builds; collect telemetry on fallback usage and handshake failures.
- Remove plaintext fallback once clients >90% adoption and publish migration notes.

## Phase 1 Implementation Notes (2025-02-15)
- `beach-human` now seals WebRTC offers, answers, and ICE candidates whenever a session passphrase is present (the original `BEACH_SECURE_SIGNALING` rollout flag has been retired). The data channel handshake stayed plaintext until Phase 2 landed.
- `beach-road` stores the new ciphertext envelopes transparently; plaintext fields are retained for backward compatibility but are blanked when sealing is active.
- Secure signaling is off by default to avoid breaking older clients (including the web UI). Operators should enable the flag only when both peers run a build with this feature.
- Transport-layer encryption remains pending (Phase 3); web parity follows in Phase 4.
- **Next checkpoints:** (a) wrap terminal traffic with the derived keys (completed in Phase 3); (b) port sealed signaling + handshake to beach-web so the feature flags can be enabled for mixed-client sessions.

## Phase 2 Implementation Notes (2025-02-15)
- `beach-human` spins up a dedicated `beach-secure-handshake` data channel whenever a passphrase is present. Both roles run a Noise `XXpsk2` exchange seeded by the Argon2id-derived key and handshake metadata (the legacy `BEACH_SECURE_TRANSPORT` flag is no longer required).
- The handshake yields symmetric send/receive keys (stored on the `WebRtcConnection`) plus a 6-digit verification string exposed via connection metadata (`secure_verification`) for hosts; values aren't yet used to wrap traffic.
- Failures (or timeouts) abort the WebRTC negotiation so sessions don't silently fall back to plaintext once secure transport is requested.
- The handshake currently skips DTLS exporter binding pending support in `webrtc-rs`; the session/peer IDs are mixed into the Noise prologue as an interim safeguard.

## Phase 3 Implementation Notes (2025-02-15)
- `WebRtcTransport` now wraps all outbound/inbound frames with ChaCha20-Poly1305 when secure transport is active. Nonces use a per-direction counter (encoded alongside the ciphertext), and a fixed AAD binds frames to the session context.
- Decryption enforces strict counter monotonicity, so replayed or reordered frames are discarded before they reach the terminal pipeline.
- Secure transport is enabled automatically once the Noise handshake completes on both offerer and answerer paths; plaintext frames are only exchanged during the legacy readiness sentinel.
- Rekey-on-volume/time and web-client parity remain open; Phase 4 will handle browser support and rollout.

## Phase 4 – Web Client Parity & Rollout Plan
### A. Web Client Cryptography
- [x] **Shared key derivation:** Introduce `transport/crypto/sharedKey.ts` in `beach-web` mirroring the Argon pipeline (currently PBKDF2 placeholder, swap to Argon2id when the WASM build lands) plus HKDF helpers for sealing/transport keys.
- [x] **Sealed signaling:** Encrypt/decrypt SDP and ICE payloads in `beach-web/src/transport` using the shared utilities and ChaCha20-Poly1305 envelopes that match the CLI implementation.
- [ ] **Noise handshake (browser):** Implement the `beach-secure-handshake` data channel logic in TypeScript using a Noise JS/WASM library, preserving the prologue inputs and verification display.
  1. Evaluate candidate Noise libraries (`@chainsafe/libp2p-noise`, `noise-c.wasm`, etc.) and prototype initiator/responder flows.
  2. Encapsulate the handshake in `transport/crypto/noiseHandshake.ts` so both offerer/answerer call a shared helper returning `{ sendKey, recvKey, verificationCode }`.
  3. Wire handshake lifecycle into `transport/webrtc.ts`, surface verification strings in the UI, and emit telemetry for success/failure paths.
- [ ] **Transport wrapper:** Reuse the ChaCha20-Poly1305 framing on the web data channel; maintain counter state and reject malformed frames.

### B. Feature Negotiation & Compatibility
5. Extend signaling metadata to advertise secure-capable clients; add CLI/browser checks to decide whether to require sealing or fall back.
6. Provide telemetry hooks (CLI + web) to emit success/failure metrics and reasons (version mismatch, handshake timeout, replay detected).

### C. Rollout & Ops
7. Add CLI/web flags to force or disable secure transport during rollout; document env vars.
8. Update operator docs with migration steps: enable flags in staging, monitor telemetry, schedule cutoff for plaintext fallback.
9. After adoption >90%, remove plaintext code paths and enforce sealed signaling/transport by default.

### D. Testing
10. Cross-client integration tests (CLI↔Web) covering secure handshake, fallback, and error handling.
11. Browser-focused fuzzing or property tests on ciphertext parser to match CLI coverage.

## Open Questions
- Can we expose DTLS exporter values from the current `webrtc-rs` crate, or do we need to patch the dependency?
- Do we keep passphrase verification on `beach-road` long-term, or switch completely to PAKE so the server never sees the secret?
- What UX do we present if the Noise handshake fails mid-session (auto-retry vs hard fail)?

## References
- `docs/zero-trust-webrtc-spec.md` — earlier notes on sealed signaling and Noise.
- `docs/secure-webrtc/dual-channel-implementation-plan.md` — dual-channel roadmap we will align with.
- Noise Protocol Framework: https://noiseprotocol.org/noise.html
- OPAQUE/PAKE background: CFRG RFC 9380.

## Immediate Next Steps (handoff summary)
1. **Browser Argon2 parity (Phase 4/A1):** replace the PBKDF2 placeholder in `derivePreSharedKey` with the same Argon2id settings used by the Rust clients and host.
2. **Browser Noise handshake (Phase 4/A3):** choose a Noise JS/WASM implementation, wire it into `transport/crypto/noiseHandshake.ts`, and surface verification strings/telemetry in the UI.
3. **Browser transport AEAD (Phase 4/A4):** reuse the ChaCha20-Poly1305 framing on the web data channel once handshake keys are available.
4. **Feature negotiation & telemetry (Phase 4/B):** advertise secure capability, add metric hooks, and gate rollout behind flags before removing plaintext fallbacks.

## Phase 4/A1 – Browser Argon2 Parity Execution Plan

### Summary
Bring `apps/beach-web` in line with the Rust toolchain by deriving sealed-signaling keys with Argon2id (64 MiB, 3 passes, parallelism = 1, 32-byte output). This removes the PBKDF2 stopgap and ensures browser clients can decrypt the CLI’s sealed SDP/ICE payloads.

### Deliverables
- WASM-backed Argon2id helper that exposes `derivePreSharedKey(passphrase, handshakeId) -> Promise<Uint8Array>`.
- Updated `sharedKey.ts` to call the helper, including lazy initialisation and structured error reporting.
- Unit and integration tests asserting byte-for-byte parity with the Rust implementation.
- Documentation and rollout notes covering bundle impact, loading strategy, and ops toggles.

### Work Breakdown
1. **Library selection & integration**
   - Adopt `argon2-browser` (preferred) or build a custom WASM module if evaluation fails. Validate that the bundled WASM supports Argon2id with configurable memory, iterations, and output length.
   - Add the dependency to `apps/beach-web` and configure Vite to emit the WASM artifact alongside the JS chunk (ensure `vite.config.ts` includes `wasm` in asset handling).
   - Provide a thin loader (`transport/crypto/argon2.ts`) that initialises the WASM once, caches the promise, and surfaces a typed API.

2. **Derivation helper implementation**
   - Replace `derivePreSharedKey` in `sharedKey.ts` to call the loader with parameters matching the Rust defaults (`memoryCost: 65536`, `timeCost: 3`, `parallelism: 1`, `hashLen: 32`, `type: Argon2id`).
   - Maintain the existing async contract and return a `Uint8Array` without base64 conversion.
   - Emit `console.error('[beach-web] argon2 derive failed', err)` before rethrow so the UI surfaces actionable errors during rollout.

3. **Parity verification**
   - Add Jest/Vitest unit tests that compare the browser-derived output against known vectors exported from the Rust side (`derive_pre_shared_key`), covering different passphrase lengths and salts.
   - Introduce an integration test (Playwright or Vitest environment) that runs the sealed signaling flow against the Rust CLI to confirm offers unwrap successfully.

4. **Telemetry & fallback handling**
   - Count Argon2 failures separately in telemetry (`secure-signaling.argon2_failure`) and surface a UI hint that suggests retrying without secure mode only if the user explicitly overrides it.
   - Keep the plaintext escape hatch (`VITE_ALLOW_PLAINTEXT=I_KNOW_THIS_IS_UNSAFE`) documented but make sealed signaling the default path once Argon2 is stable.

5. **Bundle & performance auditing**
   - Measure initial load and derive latency on target devices (M-series Mac, mid-tier Windows laptop, Chromebook). Record numbers in this file and set acceptance thresholds (≤150 ms derivation on desktop, ≤400 ms on Chromebook).
   - Investigate lazy-loading the WASM module only when a passphrase is present to avoid penalising open sessions without a shared secret.

6. **Rollout plan**
   - Behind a feature flag (`VITE_ENABLE_SECURE_SIGNALING=1` during dogfood) to allow quick rollback.
   - After staged rollout, deprecate PBKDF2 fallback code paths and update CLI/web compatibility matrix.

### Open Questions
- **WASM initialisation:** Should we prefetch the WASM binary on login, or rely on dynamic import the first time a sealed session is joined?
- **Worker isolation:** Do we need to run Argon2 in a Web Worker to avoid blocking the UI thread on low-end devices?
- **Shared module reuse:** If we add more crypto to the browser bundle (Noise, secure transport), can we consolidate into a single WASM package?

### Decision Log
- Pending: confirm `argon2-browser` meets bundle-size and WASM-streaming constraints; otherwise fall back to a custom WASM build using the existing Rust Argon2 crate compiled via `wasm-pack`.

## Phase 4/A3 – Browser Noise Handshake Execution Plan
- Select a Noise library that supports XXpsk2 with PSK injection and small bundle size; evaluate `@chainsafe/libp2p-noise` versus `noise-c.wasm` and record the decision.
- Implement `transport/crypto/noiseHandshake.ts` to drive initiator/responder flows over `beach-secure-handshake`, matching CLI prologue inputs and replay guards.
- Gate payload-channel readiness on handshake completion so send/receive keys plus the verification string land in shared `SecureTransportState` before plaintext flows.
- Surface the verification string in the web UI (host and participant), add a toast/log entry, and collect handshake latency/failure telemetry.
- Add Jest fixtures using captured CLI transcripts and a headless browser integration run against the CLI to confirm interoperable handshakes.

## Phase 4/A4 – Browser Transport AEAD Execution Plan
- Build `transport/crypto/secureDataChannel.ts` to wrap `RTCDataChannel` with ChaCha20-Poly1305 using per-direction counters derived from the handshake keys.
- Align nonce format with the CLI (96-bit counter plus channel label), trigger HKDF rekey after message/elapsed thresholds, and persist counter state across reconnects.
- Update `transport/index.ts` to swap in the secure wrapper once the handshake resolves, keeping the legacy plaintext path behind rollout flags.
- Emit replay/tamper counters and wire a temporary kill switch (`BEACH_DISABLE_SECURE_TRANSPORT`) to simplify debugging during rollout.
- Cover nonce rollover, tamper detection, and CLI↔browser ciphertext compatibility with unit tests plus end-to-end automation.

## Phase 4/B – Feature Negotiation & Telemetry Rollout
- Advertise a `secure_transport_capable` bit in signaling metadata; require mutual support before enforcing sealed signaling/transport on beach-road.
- Ship CLI/web flags (`BEACH_FORCE_SECURE`, `BEACH_DISABLE_SECURE`) and UI affordances that display the negotiated mode and any fallback reason.
- Emit structured telemetry for handshake failure classes (library init, timeout, verification mismatch) and feed them into existing Grafana alerts.
- Draft an ops checklist covering staged rollout, adoption guardrails, and the timeline for removing plaintext fallbacks; publish in operator docs.
- Queue a removal PR once adoption crosses the target threshold and add a CHANGELOG reminder so downstream integrators prepare for the cutoff.

## Phase 4/A3 Progress Notes (2025-10-11)
- Adopted `noise-c.wasm` for the browser handshake to stay aligned with the CLI’s `snow` implementation. It is the only JS option we found with first-class `XXpsk2` and PSK injection; bundle size lands at ~240 KB pre-minify and loads via Vite using an explicit `locateFile` + preloaded WASM binary shim.
- Implemented `runBrowserHandshake` in `apps/beach-web/src/transport/crypto/noiseHandshake.ts`, mirroring the CLI prologue, channel binding, and HKDF derivation. The helper drives the data-channel handshake loop, returns `{ sendKey, recvKey, verificationCode }`, and reuses pre-shared keys derived during sealed signaling when available.
- Added `SecureDataChannel` to wrap WebRTC payloads with ChaCha20-Poly1305 (`apps/beach-web/src/transport/crypto/secureDataChannel.ts`). The wrapper enforces versioned framing (`1 || counter_be || ciphertext`), maintains monotonic counters, and surfaces decrypted `message` events to the existing transport layer.
- Integrated the handshake and wrapper into `connectWebRtcTransport`: answerer now waits for `beach-secure-handshake`, runs `runBrowserHandshake` as the responder, and only exposes the terminal channel once the verification code and AEAD keys are live.
- Surfaced secure status + SAS in `BeachTerminal` so hosts/participants see “Secure” or “Plaintext” badges alongside the 6-digit verification code; the badge updates via the new `secure` transport event.
- Emitting structured telemetry to `/telemetry/secure-transport` for success/failure/fallback paths (latency, role, handshake id, reason) to feed the Grafana alerts.
- Introduced a Vitest `noiseHandshake` suite that exercises the initiator/responder flow and round-trips a secure payload through paired mock data channels. The test auto-skips if the WASM binary cannot be loaded by the Node test runner, matching the current CI constraints.

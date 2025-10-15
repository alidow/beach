# Session Create Performance Improvement Plan

## Context
- Viewer traces show a ~3 s gap between join acceptance and WebRTC offer availability.
- Host-side logs confirm the offerer spends ~1.5 s deriving the Argon2 pre-shared key and another ~1.5 s sealing the SDP before posting it.
- This work blocks the offer pipeline, so browsers poll `/webrtc/offer` repeatedly before an offer exists.

## Goals
- Begin Argon2 derivation immediately after the handshake ID is minted and run it off the async executor.
- Reuse the derived key for both offer and answer sealing without repeating the expensive stretch.
- Maintain existing secure signaling guarantees and keep the codepaths compatible with plaintext mode.

## Plan
1. **Shared Key Cell**
   - 🚧 Session-level caching is wired through the host, but currently disabled to preserve compatibility with existing clients.
   - 🚧 Once client rollout is ready, re-enable HKDF fan-out so we can stop re-running Argon2 per handshake.

2. **Async Derivation Helper**
   - ✅ Session-level stretches now run inside `spawn_blocking`, keeping the async runtime responsive.
   - ✅ Failures propagate as `TransportError::Setup`, preserving existing error handling.

3. **Payload Sealing Updates**
   - 🚧 Host/client still rely on per-handshake Argon2 until HKDF rollout lands; structure is in place for the follow-up.

4. **Answer Path Reuse**
   - 🚧 Pending HKDF rollout; current implementation falls back to legacy flow for compatibility.

5. **Diagnostics**
   - ✅ Added trace markers (`via: 'session_base'`) to confirm when cached material is used.

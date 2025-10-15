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
   - ✅ Added a session-scoped `OnceCell<Arc<[u8; 32]>>` so we only stretch the passphrase once per session.
   - ✅ Handshake-specific keys now fan out from this cached base via HKDF, eliminating duplicate Argon2 work.

2. **Async Derivation Helper**
   - ✅ Session-level stretches now run inside `spawn_blocking`, keeping the async runtime responsive.
   - ✅ Failures propagate as `TransportError::Setup`, preserving existing error handling.

3. **Payload Sealing Updates**
   - ✅ `payload_from_description`/`session_description_from_payload` accept precomputed keys and reuse them across offer/answer paths.
   - ✅ ICE candidate sealing/decryption now taps the cached handshake key as well.

4. **Answer Path Reuse**
   - ✅ Answer-side signaling and ICE decryption reuse the cached session key, skipping redundant Argon2 work.

5. **Diagnostics**
   - ✅ Added trace markers (`via: 'session_base'`) to confirm when cached material is used.

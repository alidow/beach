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
   - Introduce a `tokio::sync::OnceCell<Arc<[u8; 32]>>` per handshake to store the derived key.
   - Spawn a background task as soon as the handshake ID is created to fill the cell with `derive_pre_shared_key` output.

2. **Async Derivation Helper**
   - Wrap the Argon2 call in `tokio::task::spawn_blocking` to avoid tying up the async runtime thread.
   - Surface errors as `TransportError::Setup` so existing callers handle failures uniformly.

3. **Payload Sealing Updates**
   - Extend `payload_from_description` to accept an optional precomputed key.
   - For secure sessions, await the cell before sealing; fall back to on-demand derivation only if the cell is empty.

4. **Answer Path Reuse**
   - Plumb the shared key through both the offer and answer code paths so we never recompute within the same handshake.

5. **Diagnostics**
   - Add tracing around the background derivation to confirm overlaps with the rest of the negotiation pipeline.

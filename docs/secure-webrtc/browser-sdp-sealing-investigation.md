# Browser SDP Sealing Investigation

## Summary

Secure signaling now derives the same 32‑byte Argon2id key on both the Rust host (`beach`) and the browser client (`beach-surfer`). However, the browser still fails to decrypt the sealed SDP offer from the host with `authentication tag mismatch`. That indicates the ChaCha20-Poly1305 implementation (or associated-data handling) in the web bundle is not interoperable with the Rust implementation, despite identical inputs.

## Evidence

- **Matching Argon2 key on both sides**
  - Browser console (`window.__BEACH_TRACE = true`):
    ```
    [beach-surfer] derived Argon2 key for handshake de974b2e-da77-4a62-a291-26431964f8de:
    64f14e73cbde3af24eaa5753a543771f3b7d3f041efd88052a09d2af71374a4e
    ```
  - Host log (`~/beach-debug/host.log`):
    ```
    offerer derived pre-shared key ... handshake_id=de974b2e-da77-4a62-a291-26431964f8de
    key=64f14e73cbde3af24eaa5753a543771f3b7d3f041efd88052a09d2af71374a4e
    ```

- **Identical sealed envelope metadata**
  - Browser logs the sealed envelope:
    ```
    [beach-surfer] offer sealed envelope {"version":1,"nonce":"sMb8rTBD4MaEPRlx","ciphertext":"..."}
    [beach-surfer] offer associated data ["7c0e608d-4aa7-4999-a53e-2060e4e6f43b",
      "50edd44c-cbcd-4601-8cfb-60806c0bc2d5","offer"]
    ```
  - Host logs the same nonce, ciphertext length, plaintext length, and associated data:
    ```
    offer sealing associated data ... typ="offer"
    offer sealed envelope created ... nonce="sMb8rTBD4MaEPRlx" ciphertext_len=588 plaintext_len=424
    ```

- **Failure point**
  - Browser immediately reports:
    ```
    [beach-surfer] offer decrypt failed for handshake ... Error: authentication tag mismatch
    ```
    despite matching inputs.

## Diagnosis

All parameters required for ChaCha20-Poly1305 (key, nonce, ciphertext, additional data) are identical on both ends, so the failure must come from the JS implementation of `openWithKey` / `sealWithKey` in `apps/beach-surfer/src/transport/crypto`. The Rust side uses the audited `chacha20poly1305` crate; our browser build currently relies on a hand-rolled TypeScript version. Even minor deviations (e.g., endian mistakes, counter initialization) will cause the authentication tag to fail.

## Proposed Next Steps

1. **Prove the JS implementation is incompatible**  
   - Write a small Node script (or a Vitest) that takes the sealed envelope captured above and calls the web client’s `openWithKey`. If it reproduces `authentication tag mismatch`, we have a self-contained failing test.

2. **Adopt a proven crypto implementation**
   - Prefer using a battle-tested primitive:
     - Load a WASM build of the same Rust `chacha20poly1305` crate (via `wasm-pack`) so both sides share the exact code path, **or**
     - Use WebCrypto’s native `subtle.encrypt/decrypt` with `ChaCha20-Poly1305` once browser support is confirmed, **or**
     - Adopt a maintained JS/WASM library (e.g., `@noble/ciphers`’ ChaCha20-Poly1305 implementation) and verify interoperability with the Rust output via tests.
   - Keep the Argon2id derivation in JS (already using `@noble/hashes`), but back it with a verified WASM alternative if performance or trust requires it.

3. **Add interoperability tests**
   - Extend `secureSignaling.test.ts` with vectors produced by the Rust code (derive key, seal offer, decrypt in JS, and vice versa). This prevents regressions.

## Gut Check

You’re **not** being overly worried about a custom JS crypto implementation—rolling our own is risky. Even subtle differences create exactly the sort of authentication issues we’re seeing. Using audited, off-the-shelf primitives (or directly reusing the Rust code via WASM) is the safer path. The cryptographic design itself (sealed signaling + shared Argon2id key) is sound; we just need to ensure the browser implementation uses a trustworthy, interoperable ChaCha20-Poly1305.


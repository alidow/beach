# Noise Handshake PSK Support Plan

## Context

- The browser currently loads `noise-c.wasm` v0.4.0 (compiled from `rweather/noise-c`), while the Rust host uses the `snow` crate for Noise.
- We attempted to run the secure handshake with `Noise_XXpsk2_25519_ChaChaPoly_BLAKE2s`, matching the Rust configuration, so the pre-shared passphrase is enforced inside the Noise handshake.
- Runtime traces and a Node-based probe (`temp/crypto-interop`) show that this WASM build only exposes the base patterns (`Noise_XX`, `IK`, etc.) plus the fallback variant—**no `psk` prefixes are compiled in**. Any attempt to construct `Noise_XXpsk*` throws `NOISE_ERROR_UNKNOWN_NAME`.
- Because Rust happily supports the PSK variants, the inconsistency is entirely on the browser side.

## Goals

1. Ship a robust “application-enforced PSK” flow using the base `Noise_XX` pattern only—no custom WASM rebuilds or forks.
2. Preserve zero-trust against a compromised beach-road server (sealing + transport keys must still be derived from the shared passphrase).
3. Harden the post-handshake verification so wrong passphrases fail fast at the application layer.
4. Add automated compatibility checks between JS/WASM and Rust so crypto mismatches are caught immediately.

## Current Findings

| Pattern | noise-c.wasm v0.4.0 | rust `snow` |
|---------|--------------------|-------------|
| `Noise_XX_25519_ChaChaPoly_BLAKE2s` | ✅ supported | ✅ supported |
| `Noise_XXpsk{0,1,2,3}_25519_ChaChaPoly_BLAKE2s` | ❌ `NOISE_ERROR_UNKNOWN_NAME` | ✅ supported |
| `Noise_XXfallback_25519_ChaChaPoly_BLAKE2s` | ✅ supported | ✅ supported |
| AES-GCM / SHA256 / Curve448 variants | ✅ supported | ✅ supported |

Node helper used:

```ts
const module = await loadNoiseModule();
for (const name of candidates) {
  try {
    new module.HandshakeState(name, module.constants.NOISE_ROLE_INITIATOR).free();
  } catch (error) {
    console.log(name, error.message);
  }
}
```

## Implementation Plan (Application-Enforced PSK)

1. Standardise on `Noise_XX_25519_ChaChaPoly_BLAKE2s` for both browser and Rust.
2. Strip the `.psk(index, key)` call on the Rust builder and stop passing a PSK to the browser handshake initialiser.
3. Immediately after `Split()`:
   - Run HKDF with the passphrase + handshake hash to derive send/recv keys (exactly as today).
   - Derive the 6-digit verification code from the same material and surface it in the UI/logs.
   - **Fail fast**: if the local and remote verification codes do not match, close both data channels before exchanging application frames.
   - Optional hardening: send a dedicated challenge-response frame (`HMAC(passphrase_derived_key, nonce)`) right after the handshake; verify before rendering the terminal UI. This detects wrong-passphrase peers without waiting for the human to compare codes.
4. Log a prominent warning whenever verification or the challenge-response fails so incidents are visible.
5. Document the design so future contributors know the Noise PSK is handled at the application layer by choice—not due to an oversight.

## Compatibility Testing Strategy

Integrate the existing Rust ↔ JS harness (`temp/crypto-interop`) into CI:

1. Expose a simple CLI (`cargo run --manifest-path temp/crypto-interop/Cargo.toml`) that:
   - Derives the Argon2id PSK.
   - Runs the exact sealing / AAD / HKDF functions used in production.
   - Emits JSON envelopes.
2. Add vitest tests (`secureSignaling.test.ts`, `noiseHandshake.test.ts`) that call the CLI via `execFileSync`. Existing round-trip coverage already does this for ChaCha20; extend it for the Noise handshake once PSK support lands.
3. Add a Rust integration test (e.g., in `apps/beach/tests/noise_compat.rs`) that loads recorded JS artefacts and ensures `snow` decrypts them.
4. Gate CI on these compatibility tests so a future change in the wasm or Rust libs fails fast.

## Open Questions & Follow-Ups

- Finalise the UX for verification code display and challenge-response failure states.
- Confirm mobile browsers still load `noise-c.wasm` correctly in this setup.
- Track future availability of PSK-enabled builds, but there is **no plan** to fork the wasm package.

## Next Actions

1. Update browser + Rust handshake code to the non-PSK pattern and remove `.psk()` usage.
2. Implement the application-layer verification + optional challenge-response.
3. Wire the `temp/crypto-interop` harness into automated tests before touching production handshake code.
4. Update user-facing docs/release notes to explain the application-level PSK enforcement.

## Implementation Notes

- Handshake flows in Rust (`apps/beach/src/transport/webrtc/secure_handshake.rs`) and the browser (`apps/beach-web/src/transport/crypto/noiseHandshake.ts`) now standardise on `Noise_XX_25519_ChaChaPoly_BLAKE2s` and deliberately omit Noise PSK variants.
- Post-handshake verification is enforced by deriving the transport keys, a six-digit code, and a dedicated challenge key via HKDF, then exchanging an authenticated verification frame (`version | role | code | nonce | HMAC`). Any mismatch closes the channels and emits prominent warnings.
- The interop harness (`temp/crypto-interop`) exposes a `noise-handshake` CLI, and Vitest coverage (`noiseHandshake.interop.test.ts`) keeps the JS and Rust derivations aligned.
- These docs capture that the PSK enforcement is intentionally handled at the application layer until a PSK-capable WASM build exists.

---

Prepared so another engineer can pick this up without repeating the investigation that led to the current understanding.

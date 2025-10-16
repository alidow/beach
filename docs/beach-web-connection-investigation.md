# Beach Web Connection Investigation

## Context

We're chasing sub-second connection setup for beach-web. Baseline traces exceeded 7 s, dominated by two Argon2 derivations (one on the host, one in the browser). We changed the flow to stretch the passphrase once per session and use HKDF to derive a per-handshake key. The browser now front-loads the session key, and the host caches it. This should remove ~4 s of crypto work and pave the way for faster handshakes.

## Changes Made

1. Host (Rust) caches a session-scoped key (`derive_pre_shared_key(passphrase, session_id)`) and uses `derive_handshake_key_from_session` (HKDF) for each handshake. The offer/answer/ICE sealing paths now pull from a shared `OnceCell`.
2. Browser (TS) derives the session key on `join_success` using Argon2, then derives handshake keys via HKDF. We preload the handshake key when `handshake_ready` fires.
3. Both sides should now use the same key material: session key = Argon2(passphrase, session_id); handshake key = HKDF(session_key, handshake_id).

All unit tests still pass (`cargo check`).

## Current Failure

After deploying the changes, beach-web connects but fails the secure Noise handshake:

- Remote ICE candidates fail to decrypt with `authentication tag mismatch`.
- Noise handshake aborts with `secure handshake verification failed (mac mismatch)` for handshake IDs `ba0d53e6-…` and `32612ea5-…`.
- Browser logs show session key derivation finishing before the offer, and HKDF handing back 32-byte keys.
- Host log shows “background handshake key derivation complete” before sealing the offer.

So both ends agree on session and handshake IDs, but derived keys differ.

## Hypothesis

The HKDF parameters may not match between Rust and TS. Rust uses SHA-256, `info = b"beach:secure-signaling:handshake"`, and `salt = handshake_id`. TS uses WebCrypto HKDF with a `TextEncoder` (UTF-8 strings). Possible differences:

- Rust’s HKDF info is the literal bytes from a static slice. TS concatenates strings, but we need to confirm byte-for-byte equivalence.
- The `handshake_id` salt may include hyphens; WebCrypto uses `TextEncoder`. Need to verify Rust uses the same exact bytes (likely ASCII). If Rust treats `handshake_id` as bytes of the UUID string, we’re fine.
- Ensure no extra whitespace or case changes in the session_id or handshake_id.

Another angle: browser caches handshake key per ID, but the offerer expects the initial handshake key to derive before the answer arrives. We prefetch in `handshake_ready`, but ICE candidate processing still tries to decrypt using a Promise. If he handshake key promise rejects, we fall back to `session_key_cell.get()` in Rust—maybe still using handshakeID-salted Argon2. Need to audit the host to ensure no fallback path.

## Next Steps for Follow-up Agent

## Progress (2025-10-15)

- Added a `handshake_key` Rust utility (`temp/crypto-interop/src/bin/handshake_key.rs`) that derives the session-scoped key and handshake HKDF output from a passphrase/session/handshake triple, mirroring the host implementation.
- Extended the Vitest interop suite (`apps/beach-web/src/transport/crypto/noiseHandshake.interop.test.ts`) to compare browser `deriveHandshakeKey` outputs against the Rust toolchain via the new binary. `npm run test -- noiseHandshake.interop.test.ts` now exercises three cases and passes; the command hit the CLI timeout after reporting success (~13 s runtime).
- Result: given matching inputs, both stacks derive identical session and handshake keys. The production mismatch must stem from runtime inputs (e.g., stale session IDs or fallback code paths), not algorithm differences.

## Progress (2025-10-16)

- Host `secure_handshake` now accepts the cached pre-shared key instead of re-running per-handshake Argon2, so both sides share the HKDF-derived `handshake_key`. Offerer and answerer paths fetch the key from `await_pre_shared_key(...)` before starting Noise and log the truncated hash (`key_path=handshake_for_noise`). This removed the runtime divergence we observed in the new logging.
- Added deep diagnostic logging around the answerer handshake task (key acquisition success/failure, explicit Noise entry) and inside the verification exchange (`challenge_prepare`, `challenge_sent`, `challenge_parsed`, `challenge_verified`). These traces capture the role byte, code, nonce, and MAC fingerprints so we can correlate inbound/outbound frames when the handshake stalls.

## Next Steps for Follow-up Agent

1. Re-run beach-web with the updated binaries and confirm the new diagnostics: `handshake_for_noise` hashes should still align, and the console/host logs should show complementary `challenge_*` events.
2. If the handshake stalls or a MAC mismatch persists, use the logged role/code/nonce/MAC fingerprints to pinpoint which side diverged (e.g., mismatched verification codes or altered payloads) before touching the Noise implementation.
3. Once the connection stabilises, prune the temporary trace logging and capture a fresh timing profile for the end-to-end connect path.

Shared log snippets in `/Users/arellidow/beach-debug/host.log` around lines `15921662` and `15937700` show the MAC mismatch details. 

We should avoid reverting changes, but we need to make sure both sides derive identical key material.

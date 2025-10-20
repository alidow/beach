# 2025-10-20 Rust CLI Join: Secure Transport Counter Mismatch

## Summary
- Starting a local host with `cargo run` and attaching the Rust CLI via `cargo run -- join … --passcode` leaves the client stuck on `Connected - syncing remote session…`.
- The host logs show `failed to decode message`, `failed to decrypt inbound frame … secure transport counter mismatch`, and eventually `offerer missing data channel readiness sentinel`.
- The first encrypted `__ready__` sentinel sent by the client races ahead of the host’s call to `transport.enable_encryption`, so the host treats it as plaintext, drops it, and the AEAD counters fall out of sync.
- Once counter 0 is discarded, every subsequent frame (counters 1, 2, …) trips the mismatch guard in `EncryptionManager::decrypt`, so the readiness handshake never completes.

## Reproduction
1. Launch the beach host in one terminal:
   ```bash
   cargo run
   ```
2. In a second terminal, join with the Rust CLI (replace IDs/passcode with the values printed by the host):
   ```bash
   cargo run -- --log-level trace --log-file ~/beach-debug/client.log \
     join <session-id> --passcode <passcode>
   ```
3. Observe the CLI stay on `Connected - syncing remote session…` while the host prints the warnings shown below.

## Observations
- Host stdout (abridged):
  ```
  2025-10-20T15:49:39.538982Z WARN failed to decode message transport_id=TransportId(1) frame_len=47
  2025-10-20T15:49:39.540898Z WARN failed to decrypt inbound frame transport_id=TransportId(1) error=transport setup failed: secure transport counter mismatch
  2025-10-20T15:49:39.541075Z WARN failed to decrypt inbound frame transport_id=TransportId(1) error=transport setup failed: secure transport counter mismatch
  2025-10-20T15:49:50.029965Z WARN closing peer connection: did not receive __ready__ sentinel transport_id=TransportId(1)
  ```
- Client trace (`~/beach-debug/client.log`) shows the WebRTC transport polling for frames and timing out, never receiving terminal state updates.

## Analysis
1. `EncryptionManager::decrypt` refuses to process a frame unless its counter matches the locally tracked `recv_counter` (starts at 0, increments per frame). See `apps/beach/src/transport/webrtc/mod.rs:261-301`.
2. On the offerer (host) side, we enable encryption immediately after the Noise handshake completes (`transport.enable_encryption(&result)?` at `apps/beach/src/transport/webrtc/mod.rs:1686`).
3. On the answerer (Rust CLI) side, once the handshake result is delivered, we enable encryption and *immediately* send `__ready__` over the now-encrypted data channel (`apps/beach/src/transport/webrtc/mod.rs:3026-3044`).
4. The first encrypted sentinel reaches the host before the `encryption.enabled` flag is flipped inside the data-channel handler, so the handler enters the `encryption.is_enabled() == false` branch and tries to decode the ciphertext as plaintext. That triggers the `failed to decode message … frame_len=47` warning and the frame is dropped.
5. When the next encrypted frame arrives, the host has finished enabling encryption, so decryption is attempted. However, the frame carries counter `1` while the host still expects `0`, causing the `secure transport counter mismatch` warning and another drop.
6. Because frame `0` (the sentinel) never lands, the offerer’s readiness loop eventually times out (`offerer missing data channel readiness sentinel`), closing the peer connection. The client stays in the “syncing” state because it never receives the initial terminal snapshot.

## Impact
- Rust CLI viewers cannot attach to a host that negotiates secure transport, leaving the session unusable until the host tears down the failing peer and retries (which currently requires a manual re-join).
- The warnings are noisy but do not surface a clear actionable hint for operators; they simply see the CLI hang.

## Investigation Notes (Oct 20, 2025)
- Added temporary per-frame logging inside `WebRtcTransport::on_message` together with a debug-only `BEACH_DEBUG_ENCRYPTION_DELAY_MS` hook in `EncryptionManager::enable`.
- With a 100 ms delay injected, the first inbound frames logged `encryption_active=false` and were treated as plaintext, reproducing the user’s `failed to decode` warning without any additional clients in the mix. The readiness sentinel never arrived, confirming the drop happens before the counter mismatch.
- After updating `EncryptionManager::is_enabled` to fall back to the mutex-protected cipher state (in addition to the atomic flag), the host reported `encryption_active=true` even with the artificial delay and the headless validator completed successfully. This removes the race while keeping the fast-path atomic check once the flag flips.

## Resolution
- `apps/beach/src/transport/webrtc/mod.rs`: `EncryptionManager::is_enabled` now returns `true` as soon as the cipher state is installed, even if the atomic flag has not been flipped yet. This guarantees that the very first encrypted frame (sentinel counter 0) is decrypted correctly.
- Retested the headless Rust CLI join (and with the forced 100 ms delay) to ensure the handshake completes without warnings.

## Next Steps
- Add an automated host + headless client test that asserts the encrypted `__ready__` sentinel is received and that the host log stays free of `secure transport counter mismatch`.
- Consider keeping a low-level debug hook (disabled by default) for reproducing future handshake races; the temporary delay instrumentation was removed after verification.

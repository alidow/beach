# 2025-10-20 Bootstrap Handshake Timeout

## Summary
- Local bootstrap command fails with `ssh connection closed before bootstrap handshake`.
- Remote bootstrap log created as `/tmp/beach-bootstrap-218954.json` with size `0` bytes.
- This behavior has been recurring after apparently successful fixes.
- Added a headless validation mode to the CLI (both `beach ssh --headless` and `beach join --headless`) so we can automate the host/client smoke test. It successfully exercises the handshake plumbing but currently fails because the answerer never receives the encrypted `__ready__` sentinel.

## Remote Observations (Oct 20, 2025)
- `/tmp/beach-bootstrap-218954.json` (from the failed 11:37 UTC attempt) was zero bytes while earlier successful runs left multi-MB traces, so the background host process terminated before emitting any log output.
- `ps -eo pid,state,etime,cmd | grep "./beach host"` showed ~14 long-lived background hosts dating back to Oct 18. That indicates bootstrap clean-up has been skipping the remote process, letting it accumulate across deploys.
- The latest binary on the host is `/home/ec2-user/beach` (timestamp Oct 20 11:37), confirming the new build copied correctly before the failure.
- Running `nohup env RUST_LOG=trace BEACH_LOG_LEVEL=trace ./beach host --bootstrap-output=json ...` immediately after connecting reproduced two behaviors:
  * **Before cleanup**: the new background process exited within ~1 s with exit status `101` and left an empty `/tmp/beach-bootstrap-*.json` (matching the failure signature).
  * **After cleanup** (killing stale hosts and deleting old `/tmp/beach-bootstrap-*.json` files): the same command produced a 30 KB log almost instantly and stayed alive, as expected.
- Stress-testing with 15 fresh bootstrap launches after cleanup succeeded (each left a ~470 B JSON envelope), which suggests the earlier empty-file behavior was environmental, not deterministic in the binary.
- During the 15:03 UTC regression test the host completed the Noise handshake but immediately logged `WARN … failed to decrypt inbound frame … secure transport counter mismatch` and then `offerer missing data channel readiness sentinel`, while the client reported “Connected - syncing remote session…” and streamed no frames. This points to a second-order issue in the secure transport layer (likely reuse of cached handshake keys leaving AEAD counters out of sync) that was uncovered once bootstrap itself was stable.

## Hypotheses
- The remote host may have been operating under resource pressure (numerous sleeping host processes plus multi-GB bootstrap logs under `/tmp`). In that state, the freshly launched host exits early with error code `101` without emitting logs, producing the zero-byte bootstrap file seen by the CLI.
- Because the CLI only waits 2 s then `cat`s the bootstrap file, an immediate exit leaves the file empty and manifests as “ssh connection closed before bootstrap handshake”.
- Once stale hosts/logs were removed, repeated manual runs succeeded consistently, supporting the theory that cleanup alleviated the crash path rather than a deterministic code regression.
- The surviving AEAD counter mismatch is unrelated to the original zero-byte bootstrap but prevents the client from completing WebRTC setup; the offerer starts draining data-channel frames before it flips `encryption_enabled=true`, so the first encrypted `__ready__` sentinel is misparsed as plaintext, dropped, and every subsequent encrypted frame reports a counter mismatch.
- Oct 20 17:53 UTC headless SSH join (session 67ff4000-2fa3-412f-8b70-cd2176a4ae23) still failed: host log `/tmp/beach-bootstrap-236897.json` shows the encrypted `__ready__` frame arriving while `encryption_enabled=false`, triggering `failed to decode message` and `secure transport counter mismatch`; client log `/Users/arellidow/beach-debug/client.log` confirms the answerer sent the sentinel immediately after enabling encryption.
- Recent headless runs (Oct 20 16:00-16:20 UTC) show:
  * Offerer sends encrypted `__ready__` (sequence increments, outbound buffer grows) but answerer poll loop always times out with `offerer missing data channel readiness sentinel`.
  * Host log records repeated `failed to decode message...` warnings for frames of length 47/41 immediately before the timeout. These correspond to the encrypted `__ready__` payload being unreadable on the offerer side.
  * No `failed to decrypt` warning appears, which means the data-channel handler decrypted successfully but `decode_message` rejected the bytes (likely because we are sending plaintext "__ready__" while encryption is enabled).
  * The encrypted frames include the AAD prefix `TRANSPORT_ENCRYPTION_AAD` (currently `b"beach::transport::webrtc"`), so any mismatch in message format ends up looking like garbage to `decode_message`.

## Reproduction & Validation
1. Build the headless-capable binary: `cargo zigbuild -p beach --release --target x86_64-unknown-linux-gnu`.
2. Reproduce the encrypted-readiness failure by stretching the offerer delay (pre-fix behaviour or for regression testing):  
   ```bash
   BEACH_LOG_LEVEL=trace BEACH_LOG_FILE=~/beach-debug/client.log \
   cargo run -p beach -- ssh ec2-user@<host> \
     --copy-binary \
     --copy-from "$(git rev-parse --show-toplevel)/target/x86_64-unknown-linux-gnu/release/beach" \
     --verify-binary-hash \
     --keep-ssh \
     --ssh-keep-host-running \
     --headless \
     --handshake-timeout 300 \
     --headless-timeout 300 \
     --dev-offer-encryption-delay 150
   ```
   Expected failure signature: host log contains `failed to decode message` / `secure transport counter mismatch` while the client log shows `secure transport counter mismatch` warnings.
3. After the fix (queuing encrypted frames), rerun the same command. Host log now includes `queued encrypted frame until transport keys are installed` followed by `flushing encrypted frames queued before enable`, and the headless validator exits successfully (e.g. session `fa7dea19-e0d7-44d7-9c80-680895c6a252`).

## Next Actions
1. **Done (Oct 20)** Add explicit cleanup into the bootstrap workflow (kill orphaned `./beach host` processes, prune stale `/tmp/beach-bootstrap-*.json`) before launching a new host — implemented in `apps/beach/src/protocol/terminal/bootstrap.rs` by prepending a cleanup preamble to the remote command (only terminates stray hosts for foreground bootstraps so multiple persistent hosts can coexist).
2. Teach the CLI bootstrapper to detect an empty bootstrap file / non-zero exit and retry while surfacing the remote stderr, rather than immediately failing with the generic “connection closed” error.
3. Instrument the host binary to log (or write a failure record) before returning `ExitCode(101)` so the cause is always visible even when it fails early.
4. Build a headless “host + non-interactive client” integration check that launches both ends, waits for the client to emit the encrypted `__ready__` sentinel (or download a terminal frame), and fails the deploy if the host reports decrypt errors or sentinel timeouts.
5. Consider extending the bootstrap wait or adding a poll loop once the early-exit path is eliminated, so slow startups don’t surface as fatal errors.
6. Debug the encrypted readiness path:
   - Added extra traces (`beach::transport::webrtc::crypto`) logging the send/receive counters and frame lengths.
   - Watching for `failed to decode` warnings in host logs tells us the offerer receives the frame but cannot parse it; next step is double-checking that the answerer encrypts using the same `TransportMessage` codec (likely we're encrypting raw `"__ready__"` text while the offerer expects a binary-encoded message).
   - Mid-term plan is to craft a minimal repro that compares the plaintext version of the sentinel with the expected encoded frame so we can confirm the mismatch before changing the protocol.
7. **Done (Oct 20)** Added a hidden `--dev-offer-encryption-delay` host flag to reproduce the race and taught `WebRtcTransport` to queue encrypted frames until the offerer enables AEAD; validated with session `fa7dea19-e0d7-44d7-9c80-680895c6a252` (150 ms delay, headless run succeeded, no counter mismatches).

## Resolution
- Offerer-side `WebRtcTransport` now defers encrypted frame processing until AEAD keys are installed: frames that look encrypted are queued while `encryption_enabled=false` and replayed immediately after `enable_encryption`, so the `__ready__` sentinel can no longer be dropped regardless of timing. The hidden `--dev-offer-encryption-delay` switch remains available for stress-testing this path.

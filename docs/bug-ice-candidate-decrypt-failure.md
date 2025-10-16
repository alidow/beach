# Bug: ICE Candidate Decryption Failure in WebRTC Transport

**Status:** Confirmed bug in current codebase
**Severity:** Critical - prevents WebRTC connections from establishing
**Affected Component:** `apps/beach/src/transport/webrtc/`
**Date Reported:** 2025-10-16

## Summary

WebRTC connections fail to establish because both host (offerer) and client (answerer) cannot decrypt each other's ICE candidates during the signaling phase. This occurs even when both sides are running identical freshly-built binaries, indicating a bug in the current encryption/decryption logic rather than a version mismatch.

## Evidence

### Host Logs (EC2 Remote)

From `/tmp/beach-bootstrap-43211.json`:

```
host_version: 0.1.0-20251016210041

2025-10-16T21:03:21.199283Z WARN sealed ice candidate decrypt failed with handshake key; falling back to passphrase
handshake_id="acac31a6-318f-4a27-b2f8-347c14eca4db"
from_peer="d0c1f489-ee4d-4e2e-a2a9-87ce9902fec0"
to_peer="384c8fca-df99-42c1-990b-d2db9b5371a4"
error=transport setup failed: secure signaling decrypt failed: aead::Error

2025-10-16T21:03:22.700257Z WARN sealed ice candidate decrypt failed with handshake key; falling back to passphrase
handshake_id="acac31a6-318f-4a27-b2f8-347c14eca4db"
from_peer="d0c1f489-ee4d-4e2e-a2a9-87ce9902fec0"
to_peer="384c8fca-df99-42c1-990b-d2db9b5371a4"
error=transport setup failed: secure signaling decrypt failed: aead::Error

datachannel open timeout, closing peer connection peer_id=e179a0a7-4de4-4296-aa6f-2aef2892b46c

peer negotiation ended with error peer_id=e179a0a7-4de4-4296-aa6f-2aef2892b46c
error=transport setup failed: offerer data channel did not open
```

### Client Logs (Local MacOS)

From `/Users/arellidow/beach-debug/client.log`:

```
2025-10-16T20:21:26.156340Z WARN sealed ice candidate decrypt failed with handshake key; falling back to passphrase
handshake_id="955b9821-bf01-4445-a996-23385f6b6197"
from_peer="bbc1b7be-b774-4e8c-8285-a38e2c46e528"
to_peer="e179a0a7-4de4-4296-aa6f-2aef2892b46c"
error=transport setup failed: secure signaling decrypt failed: aead::Error

2025-10-16T20:21:27.680742Z WARN sealed ice candidate decrypt failed with handshake key; falling back to passphrase
handshake_id="955b9821-bf01-4445-a996-23385f6b6197"
from_peer="bbc1b7be-b774-4e8c-8285-a38e2c46e528"
to_peer="e179a0a7-4de4-4296-aa6f-2aef2892b46c"
error=transport setup failed: secure signaling decrypt failed: aead::Error

2025-10-16T20:21:26.204439Z DEBUG target="beach::transport::webrtc" role="answerer"
await="transport_slot.lock" state="end" has_transport=false attempts=1

[900+ iterations of transport_slot.lock attempts, all with has_transport=false]

2025-10-16T20:22:37.359426Z WARN signaling websocket error: WebSocket protocol error: Connection reset without closing handshake
```

### Key Observations

1. **Both sides running identical binaries:**
   - Local: `beach 0.1.0-20251016210041`
   - Remote: `beach 0.1.0-20251016210041`

2. **Both sides fail to decrypt ICE candidates:**
   - Host cannot decrypt candidates from client
   - Client cannot decrypt candidates from host
   - Error is symmetric: `aead::Error` on both sides

3. **Handshake key derivation appears successful:**
   - Client log shows: `handshake key cached handshake_id=955b9821-bf01-4445-a996-23385f6b6197 key_hash=f1d84d1354690672`
   - Key is derived from session passphrase

4. **Connection sequence:**
   - ✅ Session registration succeeds
   - ✅ WebSocket signaling connection establishes
   - ✅ SDP offer/answer exchange succeeds
   - ❌ ICE candidate decryption fails
   - ❌ WebRTC data channel never opens
   - ⏱️  Client spins in `transport_slot.lock` loop waiting for transport
   - ⏱️  Host times out after 15s waiting for data channel

## User Impact

**Symptom:** Client gets stuck at "Connected - syncing remote session..." with empty terminal grid.

**Root Cause:** No data can flow because the WebRTC data channel never establishes due to ICE negotiation failure.

## How to Replicate

### Prerequisites

1. Fresh build of beach on local machine (MacOS)
2. EC2 instance running Amazon Linux 2023 (x86_64)
3. SSH access to EC2 instance
4. Redis instance for beach-road session server

### Steps

1. Build fresh release binary for Linux:
   ```bash
   cargo build --release --target x86_64-unknown-linux-musl -p beach
   ```

2. Copy binary to remote:
   ```bash
   scp -i ~/.ssh/beach-test-singapore.pem \
     target/x86_64-unknown-linux-musl/release/beach \
     ec2-user@13.215.162.4:~/beach
   ```

3. Verify both binaries have matching versions:
   ```bash
   cargo run -p beach -- --version
   # Output: beach 0.1.0-20251016210041

   ssh -i ~/.ssh/beach-test-singapore.pem ec2-user@13.215.162.4 "./beach --version"
   # Output: beach 0.1.0-20251016210041
   ```

4. Start SSH bootstrap session with trace logging:
   ```bash
   BEACH_LOG_LEVEL=trace \
   BEACH_LOG_FILE=/Users/arellidow/beach-debug/client.log \
   cargo run -p beach -- ssh \
     --ssh-flag=-i \
     --ssh-flag=/Users/arellidow/.ssh/beach-test-singapore.pem \
     --ssh-flag=-o \
     --ssh-flag=StrictHostKeyChecking=accept-new \
     ec2-user@13.215.162.4
   ```

5. Observe client stuck at "Connected - syncing remote session..."

6. Check host logs on remote:
   ```bash
   ssh -i ~/.ssh/beach-test-singapore.pem ec2-user@13.215.162.4 \
     "cat /tmp/beach-bootstrap-*.json | tail -50"
   ```

7. Examine client logs:
   ```bash
   grep "decrypt failed" /Users/arellidow/beach-debug/client.log
   grep "transport_slot.lock" /Users/arellidow/beach-debug/client.log | wc -l
   # Shows 900+ lock attempts
   ```

## Technical Details

### Encryption Flow

1. **Session key derivation:**
   - Session passphrase → SHA-256 hash → session key
   - Client log: `session key cached session_id=2a1333ce-d3b9-4a4a-8362-16e143a7ef33 session_hash=7f11572ba9f1464a`

2. **Handshake key derivation:**
   - Session key → HKDF with handshake_id → handshake key
   - Client log: `handshake key cached handshake_id=955b9821-bf01-4445-a996-23385f6b6197 handshake_hash=f1d84d1354690672`

3. **ICE candidate sealing:**
   - Candidate JSON → ChaCha20-Poly1305 AEAD encryption → SealedEnvelope
   - Associated data: `handshake_id || from_peer || to_peer || typ`

4. **ICE candidate unsealing (FAILING):**
   - SealedEnvelope → ChaCha20-Poly1305 AEAD decryption → Candidate JSON
   - **Error:** `aead::Error` indicates authentication tag verification failed

### Potential Root Causes

1. **Associated data mismatch:**
   - Encryption and decryption might be using different associated data
   - Check construction of AAD in seal vs unseal operations

2. **Key derivation inconsistency:**
   - Different peer ordering in key derivation
   - Handshake ID or peer ID mismatch between seal/unseal

3. **Nonce handling:**
   - Nonce reuse or incorrect nonce format
   - Base64 encoding/decoding issues

4. **Endianness or serialization:**
   - Different byte ordering between MacOS and Linux
   - JSON serialization differences

5. **Recent code changes:**
   - Check git history for recent changes to WebRTC sealing/unsealing logic
   - Verify any refactoring of encryption primitives

## Affected Code Paths

Key files to investigate:

1. **ICE candidate encryption:**
   - Location: `apps/beach/src/transport/webrtc/` (likely in signaling module)
   - Functions: `seal_ice_candidate()`, `unseal_ice_candidate()`

2. **Key derivation:**
   - Location: Search for `session_derived`, `handshake_pre_shared`
   - Functions: Session key HKDF, handshake key HKDF

3. **Associated data construction:**
   - Search for: `associated data`, `handshake_id`, `from_peer`, `to_peer`
   - Verify AAD matches between seal and unseal

## Workaround

None currently available. The bug prevents WebRTC transport from working entirely.

## Next Steps

1. Add detailed trace logging to seal/unseal operations:
   - Log associated data on both sides
   - Log nonce values
   - Log key hashes

2. Compare seal vs unseal operations:
   - Verify AAD construction matches exactly
   - Check peer ID ordering
   - Verify nonce encoding

3. Add integration test:
   - Test ICE candidate round-trip seal/unseal
   - Verify with different peer ID orderings
   - Test cross-platform (MacOS ↔ Linux)

4. Consider temporary fallback:
   - The code mentions "falling back to passphrase" but this doesn't seem to work
   - Investigate passphrase-based decryption path

## Related Files

- `/Users/arellidow/beach-debug/client.log` - Full client trace logs (1.9GB)
- Client session: `2a1333ce-d3b9-4a4a-8362-16e143a7ef33`
- Client handshake: `955b9821-bf01-4445-a996-23385f6b6197`
- Remote bootstrap: `/tmp/beach-bootstrap-43211.json`

## Timeline

- **Oct 16, 2025 20:14:** First observed ICE decrypt failures
- **Oct 16, 2025 20:21:** Confirmed on both sides with matching binaries
- **Oct 16, 2025 21:03:** Reproduced with fresh binary build (version `0.1.0-20251016210041`)
- **Oct 16, 2025 21:10:** Confirmed as encryption bug, not version mismatch

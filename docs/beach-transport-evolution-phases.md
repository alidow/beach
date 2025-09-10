# Beach Transport Evolution - Phased Project Plan

Status: Active  
Date: 2025-09-08  
Author: Implementation Team  

## Overview

This document outlines the phased implementation plan for evolving Beach's transport layer to support dual-channel WebRTC with zero-trust security. Each phase includes goals, implementation prompts, testing strategy, and verification methods.

### Status Snapshot
- Current: Phase 2a completed (MVP over WebSocket signaling/relay)
- Next (High Priority): Phase 2b – WebRTC dual-channel (control reliable + output unreliable)
- Done: Phase 0 (remove passphrase from signaling), Phase 1 (channel abstraction)

Notes
- MVP client/server are exchanging messages via beach-road over WebSockets using `ClientMessage::Signal` as a relay. WebRTC data channels are not yet wired to application traffic.
- We will implement WebRTC signaling through beach-road and enable dual data channels next, while keeping WebSocket fallback.

## Phase 0: Remove Passphrase from Signaling

### Goal
**CRITICAL SECURITY FIX**: Stop sending the passphrase to the untrusted beach-road signaling server. The passphrase should only be known to the client and server as a shared secret.

### Implementation Prompt
```
Read @docs/dual-channel-implementation-plan.md focusing on the "Phase 1: Transport Layer Evolution" section, specifically 1.2 Remove Passphrase from Signaling.

1. Update apps/beach/src/session/mod.rs to store passphrase locally only
2. Modify signaling messages to exclude passphrase
3. Generate session IDs independently from passphrase
4. Ensure WebRTC transport still functions without passphrase in signaling
```

### Automated Tests
- Location: `apps/beach/src/tests/transport/`
- Tests to create:
  - `test_passphrase_not_in_signaling()` - Verify passphrase never appears in Signal messages
  - `test_session_id_generation()` - Ensure session IDs are generated independently

### Human Verification
```bash
# Start beach-road
cargo run -p beach-road

# In another terminal, start a beach server
cargo run -p beach -- bash

# Use beach-road debug endpoint to verify no passphrase in server state
curl http://localhost:8080/debug | jq . | grep -i passphrase
# Should return nothing

# Check signaling messages don't contain passphrase
curl http://localhost:8080/debug/sessions | jq .
# Verify no passphrase field in any session data
```

### Commit Checkpoint
```bash
cargo test --workspace
git add -A
git commit -m "Phase 0: Remove passphrase from signaling messages

- Store passphrase locally in Session struct only
- Generate session IDs independently  
- Critical security fix: passphrase no longer sent to untrusted signaling server"
git push
```

---

## Phase 1: Channel Abstraction Layer

### Goal
Create the foundation for supporting multiple channels with different reliability characteristics.

### Implementation Prompt
```
Read @docs/dual-channel-implementation-plan.md section "1.1 Channel Abstraction".

1. Create apps/beach/src/transport/channel.rs with:
   - ChannelReliability enum (Reliable, Unreliable)
   - ChannelPurpose enum (Control, Output, Custom)
   - Channel trait with send/receive methods
2. Update Transport trait in transport/mod.rs to support multiple channels
3. Add channel management to existing transports (WebRTC first)
```

### Automated Tests
- Location: `apps/beach/src/tests/transport/channel_test.rs`
- Tests to create:
  - `test_channel_creation()` - Verify channels can be created with different reliability
  - `test_channel_purpose_routing()` - Ensure messages route to correct channel by purpose
  - `test_multiple_channels()` - Test managing multiple channels simultaneously

### Human Verification
```bash
# Build and run tests
cargo test channel

# Start beach server with debug logging
RUST_LOG=debug cargo run -p beach -- bash 2>&1 | grep -i channel
# Should see channel creation logs

# Verify compilation with new channel abstractions
cargo check -p beach
```

### Commit Checkpoint
```bash
cargo test --workspace
git add -A
git commit -m "Phase 1: Add channel abstraction layer

- Create ChannelReliability and ChannelPurpose enums
- Update Transport trait for multi-channel support
- Foundation for dual-channel architecture"
git push
```

---

## Phase 2a: Minimal Client (Vertical Slice)

### Goal
Deliver a usable end-to-end path early. Implement a minimal client that can join a session, exchange input/output, and validate routing, flow control, and UX before adding an unreliable output channel and full security.

Update (Completed)
- Implemented using WebSocket signaling + relay via beach-road. Client and server exchange application messages via `ClientMessage::Signal` routed by the session server.
- WebRTC data channels were deferred to Phase 2b; channel abstraction and WebRTC skeleton exist in code.

### Implementation Prompt
```
1. Add client mode loop in `apps/beach/src/main.rs`:
   - Parse `--join`; resolve public vs private by URL (no Clerk yet)
   - Prompt for passphrase if not provided via `--passphrase` or env
   - Connect WebSocket signaling to beach-road; use `ClientMessage::Signal` relay for app traffic (temporary MVP)
2. Implement basic client in `apps/beach/src/client/`:
   - Wire stdin → ControlMessage::TerminalInput over the relayed path
   - Render server terminal output frames from relayed messages
   - Heartbeats and simple acks (ack cadence stub)
3. Server: route protocol/app messages to the appropriate session components
4. Respect logging policy: no noisy stderr; use `--debug-log` for traces
```

### Automated Tests
- Location: `apps/beach/src/tests/client/`
- Tests to create:
  - `test_client_join_flow()` - Simulate join and control channel open (mock transport)
  - `test_input_routing()` - Verify stdin maps to control message
  - `test_output_render()` - Verify client renders received output frames

### Human Verification
```
# Start beach-road
cargo run -p beach-road

# Start beach server
cargo run -p beach -- bash

# Start minimal client in another terminal
cargo run -p beach -- --join public.localhost:8080/<session-id>
# Type in client; see input reflected on server PTY; see output rendered in client
```

### Commit Checkpoint
```
cargo test client
git add -A
git commit -m "Phase 2a: Minimal client vertical slice\n\n- Join + control channel\n- Input/output over reliable control channel\n- Basic heartbeats/acks"
git push
```

---

## Phase 2b: WebRTC Dual-Channel Implementation (Next Up)

### Goal
Implement separate reliable control channel and unreliable output channel in WebRTC transport.

Scope clarification
- Integrate WebRTC signaling via beach-road (reusing the existing WebSocket signaling transport) and transition application traffic off the WebSocket relay onto WebRTC data channels.
- Maintain WebSocket fallback (`force` flags) during rollout.

### Implementation Prompt
```
Read @docs/dual-channel-implementation-plan.md section "1.3 WebRTC Dual-Channel Support".

1. Update `apps/beach/src/transport/webrtc/mod.rs`:
   - Create two data channels: `beach/ctrl/1` (reliable, ordered) and `beach/term/1` (unreliable, unordered)
   - Route control messages to reliable channel; route terminal output to unreliable channel
   - Handle `on_data_channel` on the receiving side and map incoming channels by label to `ChannelPurpose`
2. Add remote signaling adapter (see `docs/dual-channel-webrtc-implementation.md`) to exchange SDP/ICE via beach-road
3. Update message routing in subscription/client handlers to prefer WebRTC when available; keep WebSocket as fallback
4. Ensure proper channel lifecycle management
```

### Automated Tests
- Location: `apps/beach/src/tests/transport/webrtc_dual_channel_test.rs`
- Tests to create:
  - `test_dual_channel_creation()` - Verify both channels are created
  - `test_control_channel_reliability()` - Ensure control messages never drop
  - `test_output_channel_unreliable()` - Verify output channel allows loss
  - `test_channel_message_routing()` - Test correct routing based on message type

### Human Verification
```bash
# Start beach-road
cargo run -p beach-road

# Start beach server
cargo run -p beach -- bash

# Use beach-road debug to verify dual channels
curl http://localhost:8080/debug/sessions | jq '.[] | .channels'
# Should show two channels with different reliability settings

# Monitor stats via logs for now; consider adding a debug endpoint in beach-road later
```

### Commit Checkpoint
```bash
cargo test webrtc_dual_channel
git add -A
git commit -m "Phase 2b: Implement WebRTC dual-channel support

- Create reliable control channel (beach/ctrl/1)
- Create unreliable output channel (beach/term/1)
- Route messages based on type"
git push
```

---

## Phase 3: Public Mode with Generated Passphrases

### Goal
Implement amazing UX for public sessions with auto-generated, human-friendly codes.

Note: This can be implemented in parallel with Phase 2a to validate UX with the minimal client.

### Implementation Prompt
```
Read @docs/dual-channel-implementation-plan.md "CRITICAL: Developer Experience & Modes" section on Public Beach Mode.

1. Update apps/beach/src/main.rs:
   - Generate short codes when no passphrase provided
   - Add interstitial display (press Enter or wait 60s)
2. Update apps/beach/src/session/mod.rs:
   - Implement public.<host> URL scheme
   - Add ephemeral code generation with TTL
3. Add rate limiting for generated codes
```

### Automated Tests
- Location: `apps/beach/src/tests/public_mode_test.rs`
- Tests to create:
  - `test_code_generation()` - Verify human-friendly code format
  - `test_code_uniqueness()` - Ensure codes don't repeat frequently
  - `test_public_url_format()` - Verify public.<host> URL generation

### Human Verification
```bash
# Start beach without passphrase
cargo run -p beach -- bash
# Should see generated code and interstitial prompt

# Verify URL format
# Should display: public.localhost:8080/<session-id>

# Test joining with generated code
# In the interstitial, note the code, then in another terminal:
cargo run -p beach --join <session-id> --passphrase <code>
# Should connect successfully

# Verify beach-road shows public session
curl http://localhost:8080/debug/sessions | jq '.[] | {id, is_public}'
```

### Commit Checkpoint
```bash
cargo test public_mode
git add -A
git commit -m "Phase 3: Add public mode with generated passphrases

- Auto-generate human-friendly codes
- Add interstitial display
- Implement public.<host> URL scheme"
git push
```

---

## Phase 4: Sealed Signaling Implementation

### Goal
Protect SDP and ICE candidates from tampering by sealing them with passphrase-derived keys.

### Implementation Prompt
```
Read @docs/zero-trust-webrtc-spec.md section "Sealed Signaling (SS)".

1. Add sealed signaling to apps/beach/src/transport/webrtc/sealed.rs:
   - Implement ChaCha20-Poly1305 AEAD sealing
   - Derive keys from passphrase using Argon2id
   - Create seal/unseal functions for SDP and ICE
2. Update signaling flow to use sealed envelopes
3. Add timestamp and nonce for replay protection
```

### Automated Tests
- Location: `apps/beach/src/tests/transport/sealed_signaling_test.rs`
- Tests to create:
  - `test_seal_unseal_roundtrip()` - Verify seal/unseal with correct key
  - `test_tamper_detection()` - Ensure modified sealed data fails to open
  - `test_replay_protection()` - Verify old timestamps are rejected
  - `test_argon2_derivation()` - Test key derivation from passphrase

### Human Verification
```bash
# Start beach with passphrase
cargo run -p beach -- --passphrase test123 bash

# Check beach-road shows sealed signaling
curl http://localhost:8080/debug/sessions | jq '.[] | .last_signal'
# Should see sealed envelope with "v": "ssv1", "cipher": "chacha20poly1305"

# Verify SDP is not readable in plaintext
curl http://localhost:8080/debug/sessions | jq '.[] | .last_signal | .sdp'
# Should return null (SDP is inside sealed envelope)
```

### Commit Checkpoint
```bash
cargo test sealed_signaling
git add -A
git commit -m "Phase 4: Implement sealed signaling

- Add ChaCha20-Poly1305 sealing for SDP/ICE
- Derive keys from passphrase with Argon2id
- Protect against signaling tampering"
git push
```

---

## Phase 5: Application Handshake

### Goal
Authenticate peers after WebRTC connection using Noise protocol with channel binding.

### Implementation Prompt
```
Read @docs/zero-trust-webrtc-spec.md section "Application Handshake (AKE)".

1. Add handshake to apps/beach/src/transport/webrtc/handshake.rs:
   - Implement Noise XXpsk2 pattern using snow crate
   - Mix DTLS exporter into handshake prologue
   - Gate PTY access until handshake completes
2. Run handshake on control channel after connection
3. Derive session keys from completed handshake
```

### Automated Tests
- Location: `apps/beach/src/tests/transport/handshake_test.rs`
- Tests to create:
  - `test_handshake_success()` - Verify handshake with correct passphrase
  - `test_handshake_failure()` - Ensure wrong passphrase fails
  - `test_channel_binding()` - Verify DTLS exporter is mixed in
  - `test_pty_gating()` - Confirm PTY blocked until handshake done

### Human Verification
```bash
# Start beach server with debug logging
RUST_LOG=debug cargo run -p beach -- --passphrase test bash 2>&1 | grep -i handshake
# Should see handshake initiation and completion

# Use beach-road debug to verify handshake state
curl http://localhost:8080/debug/sessions | jq '.[] | .handshake_state'
# Should show "completed" after connection

# Verify PTY not accessible before handshake
# (Would require client implementation to fully test)
```

### Commit Checkpoint
```bash
cargo test handshake
git add -A
git commit -m "Phase 5: Add application handshake

- Implement Noise XXpsk2 with channel binding
- Gate PTY access until authenticated
- Derive session keys from handshake"
git push
```

---

## Phase 6: Frame Versioning & Resync

### Goal
Enable recovery from packet loss on the unreliable output channel.

### Implementation Prompt
```
Read @docs/dual-channel-implementation-plan.md section "2.3 Server Output Processing".

1. Add frame versioning to apps/beach/src/server/terminal_state/frame.rs:
   - Add sequence numbers to output frames
   - Implement periodic snapshots
2. Add resync protocol to apps/beach/src/session/resync.rs:
   - Detect missing frames
   - Request resync over control channel
   - Send snapshot or missing frames
```

### Automated Tests
- Location: `apps/beach/src/tests/resync_test.rs`
- Tests to create:
  - `test_frame_sequencing()` - Verify frames have monotonic sequence numbers
  - `test_gap_detection()` - Detect missing frames from sequence gaps
  - `test_resync_request()` - Test resync protocol initiation
  - `test_snapshot_generation()` - Verify periodic snapshots

### Human Verification
```bash
# Start beach server
cargo run -p beach -- bash

# Generate output and check frame sequences
echo "Line 1" && echo "Line 2" && echo "Line 3"

# Use beach-road debug to see frame sequences
curl http://localhost:8080/debug/sessions | jq '.[] | .last_frames'
# Should show sequence numbers

# Simulate packet loss (would need client to fully test)
# Monitor resync requests in debug endpoint
curl http://localhost:8080/debug/sessions | jq '.[] | .resync_stats'
```

### Commit Checkpoint
```bash
cargo test resync
git add -A
git commit -m "Phase 6: Add frame versioning and resync

- Add sequence numbers to output frames
- Implement resync protocol for recovery
- Generate periodic snapshots"
git push
```

---

## Phase 7: Private Mode with Clerk Integration

### Goal
Add enterprise authentication and authorization using Clerk.

### Implementation Prompt
```
Read @docs/dual-channel-implementation-plan.md "CRITICAL: Developer Experience & Modes" section on Private Beach Mode.

1. Add login command to apps/beach/src/cli/login.rs:
   - Implement browser/device code flow
   - Store profiles in ~/.beach/config
   - Store credentials in ~/.beach/credentials
2. Add Clerk verification to apps/beach/src/auth/clerk.rs:
   - Verify JWTs using JWKS
   - Check group membership
   - Enforce authorization policies
3. Use private.<host> URL scheme for private sessions
```

### Automated Tests
- Location: `apps/beach/src/tests/auth/`
- Tests to create:
  - `test_profile_storage()` - Verify profile save/load
  - `test_jwt_verification()` - Test JWT validation with mock JWKS
  - `test_group_authorization()` - Verify group membership checks
  - `test_private_url_format()` - Check private.<host> URLs

### Human Verification
```bash
# Test login flow (with mock Clerk)
CLERK_MOCK=true cargo run -p beach login
# Should open browser or show device code

# Verify profile created
cat ~/.beach/config
cat ~/.beach/credentials

# Start private session
cargo run -p beach --profile default -- bash
# Should require authentication

# Verify private URL format
# Should display: private.localhost:8080/<session-id>

# Check beach-road shows authenticated session
curl http://localhost:8080/debug/sessions | jq '.[] | {id, auth_state, user}'
```

### Commit Checkpoint
```bash
cargo test auth
git add -A
git commit -m "Phase 7: Add private mode with Clerk integration

- Implement beach login with browser/device flow
- Add profile management (AWS CLI style)
- Enforce group-based authorization
- Use private.<host> URL scheme"
git push
```

---

## Testing Strategy Summary

### Unit Tests
- Each phase has dedicated unit tests in `apps/beach/src/tests/`
- Run with `cargo test` after each implementation

### Integration Tests
- Use existing WebRTC transport tests as baseline
- Add new integration tests for dual-channel behavior

### Manual Verification
- Before Phase 2a: use beach-road debug endpoints; curl/jq to inspect server state
- After Phase 2a: prefer live end-to-end checks using the minimal client; keep debug endpoints for introspection
- Monitor logs with RUST_LOG=debug or --debug-log for detailed tracing

### Continuous Integration
- All tests must pass before commit
- Each phase is a complete, working increment
- Feature flags for dev and CI:
  - `BEACH_SEALED_SIGNALING` (default false in dev; true in CI matrix job)
  - `BEACH_REQUIRE_HANDSHAKE` (default false in dev; true in CI matrix job post-Phase 5)
  - `BEACH_OUTPUT_UNRELIABLE` (default true; set false to force single-channel fallback)
  - `BEACH_FORCE_WEBSOCKET` (default false; set true to bypass WebRTC)
  - `BEACH_FORCE_SINGLE_CHANNEL` (default false; set true to disable dual-channel)

## Success Criteria

Each phase is considered complete when:
1. All automated tests pass
2. Manual verification shows expected behavior via beach-road debug
3. Code is committed and pushed
4. System remains functional (no regressions)

## Next Steps After All Phases

1. Harden client features (resync UI, robust input modes, performance tuning)
2. Add browser-based client support
3. Deploy to production with monitoring
4. Gather user feedback and iterate

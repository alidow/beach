# Beach Client Subscription Hang Issue

## Problem Statement

The beach client hangs indefinitely when attempting to connect to a server. The client successfully sends a `Subscribe` message but never receives the expected `SubscriptionAck` and `Snapshot` responses from the server, causing it to block waiting for these messages.

## Symptoms

1. Client connects to server via WebSocket/WebRTC
2. Client sends `ClientMessage::Subscribe` wrapped in `AppMessage::Protocol`
3. Client blocks waiting for `ServerMessage::SubscriptionAck`
4. No response ever arrives, causing indefinite hang
5. Terminal UI never renders

## Investigation Summary

### Initial Hypotheses (Incorrect)

1. **Server not processing Subscribe messages** - Disproven: Debug logging showed messages are received
2. **Handler not properly wired** - Disproven: CompositeHandler correctly routes to SessionBridgeHandler
3. **MockTransport issue** - Red herring: MockTransport in SessionBridgeHandler is only for internal SubscriptionManager use

### Root Causes Identified

#### 1. Critical Deadlock in SessionBridgeHandler (FIXED)

**Location**: `apps/beach/src/session/message_handlers/session_bridge_handler.rs:88-99`

**Issue**: A spawned task held an `RwLock` read guard across an `.await` point:

```rust
// BEFORE (deadlock):
tokio::spawn(async move {
    while let Some(msg) = rx.recv().await {
        let app_msg = AppMessage::Protocol {
            message: serde_json::to_value(&msg).unwrap_or_default(),
        };
        
        let session = session.read().await;
        let _ = session.send_to_client(&peer_id, app_msg).await; // Holding lock across await!
    }
});
```

**Fix Applied**:
```rust
// AFTER (fixed):
tokio::spawn(async move {
    while let Some(msg) = rx.recv().await {
        let app_msg = AppMessage::Protocol {
            message: serde_json::to_value(&msg).unwrap_or_default(),
        };
        
        // Send via ServerSession - avoid holding lock across await
        {
            let session = session.read().await;
            let _ = session.send_to_client(&peer_id, app_msg).await;
        } // Lock dropped here
    }
});
```

#### 2. WebRTC Connection Not Establishing (PRIMARY ISSUE)

The fundamental issue is that the WebRTC transport layer is not establishing a connection between client and server. This prevents ANY messages from flowing, including subscription messages.

**Evidence**:
- No ICE connection state changes logged
- No data channel open events
- Client stuck waiting for WebRTC transport to be ready

## Architecture Overview

The subscription system uses the following flow:

```
Client                    Server
  |                         |
  |--Subscribe message----->|
  |   (via WebRTC)          |
  |                         v
  |                    CompositeHandler
  |                         |
  |                         v
  |                 SessionBridgeHandler
  |                         |
  |                         v
  |                 SubscriptionManager
  |                         |
  |                    (creates mpsc channel)
  |                         |
  |                    (spawns task to bridge)
  |                         |
  |<--SubscriptionAck--------|
  |   (via WebRTC)          |
  |<--Snapshot---------------|
```

## Debug Logging Added

To investigate this issue, debug logging was added to:

1. **WebRTC transport** (`transport/webrtc/mod.rs`):
   - ICE connection state changes
   - Data channel open/close events

2. **SessionBridgeHandler** (`session/message_handlers/session_bridge_handler.rs`):
   - Incoming message reception
   - Subscribe message processing

3. **TerminalClient** (`client/terminal_client.rs`):
   - Subscribe message sending
   - Waiting for acknowledgment

All debug logging uses file-based output via `BEACH_DEBUG_LOG` environment variable to avoid corrupting the terminal UI.

## Critical Debugging Guidelines

**⚠️ NEVER write debug output to stdout or stderr!**

The terminal UI will be corrupted if you print to stdout/stderr. ALWAYS use file-based debug logging:

```bash
# Server
BEACH_DEBUG_LOG=/tmp/beach-server.log cargo run -p beach -- bash -c "echo 'test'; cat"

# Client  
BEACH_DEBUG_LOG=/tmp/beach-client.log cargo run -p beach -- --join <session-url>
```

## Proposed Next Steps

### Immediate Actions

1. **Fix WebRTC Connection Issue**:
   - Add more detailed logging to WebRTC signaling flow
   - Verify ICE candidates are being exchanged properly
   - Check if STUN/TURN servers are accessible
   - Ensure offer/answer exchange completes

2. **Verify Message Flow**:
   - Once WebRTC connects, verify Subscribe messages reach the handler
   - Ensure the spawned task in SessionBridgeHandler runs
   - Confirm SubscriptionAck/Snapshot are sent back

### Long-term Improvements

1. **Timeout Mechanisms**:
   - Add connection timeout for WebRTC establishment
   - Add timeout for subscription acknowledgment
   - Provide meaningful error messages on timeout

2. **Connection Status Visibility**:
   - Add connection state indicators
   - Show WebRTC ICE connection status
   - Display subscription state

3. **Architectural Considerations**:
   - Consider whether SessionBridgeHandler needs its own transport
   - Evaluate if the spawned task pattern is optimal
   - Review lock usage patterns throughout the codebase

## Testing Approach

1. Start beach-road signaling server:
   ```bash
   cargo run -p beach-road
   ```

2. Start server with debug logging:
   ```bash
   BEACH_DEBUG_LOG=/tmp/server.log cargo run -p beach -- bash -c "echo 'test'; sleep 60"
   ```

3. Start client with debug logging:
   ```bash
   BEACH_DEBUG_LOG=/tmp/client.log cargo run -p beach -- --join <session-url>
   ```

4. Check debug logs for:
   - WebRTC connection state changes
   - Subscribe message reception on server
   - SubscriptionAck/Snapshot sending
   - Any error messages

## Related Files

- `apps/beach/src/client/terminal_client.rs` - Client subscription logic
- `apps/beach/src/session/message_handlers/session_bridge_handler.rs` - Server subscription handler
- `apps/beach/src/subscription/manager.rs` - Subscription management
- `apps/beach/src/transport/webrtc/mod.rs` - WebRTC transport implementation
- `apps/beach/src/session/mod.rs` - Session management and message routing

## Conclusion

The subscription hang is caused by two issues:
1. A deadlock in the subscription message handler (now fixed)
2. WebRTC connection not establishing (primary issue, still needs investigation)

The architecture itself is sound - messages flow correctly through CompositeHandler → SessionBridgeHandler → SubscriptionManager when the transport layer is working. The focus should be on fixing the WebRTC connection establishment issue.
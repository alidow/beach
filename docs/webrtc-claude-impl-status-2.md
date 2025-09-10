# WebRTC Implementation Status Report - Phase 2

## Executive Summary

The Beach terminal sharing application has a fundamental architectural issue preventing WebRTC connections: **the server immediately executes its PTY command and terminates without waiting for client connections**. This means WebRTC initialization never occurs because no client can connect before the server exits.

## Current State

### What Works
- WebRTC transport is implemented with offer/answer/ICE handling and file-based logging hooks
- Comprehensive debug logging infrastructure is in place
- Signaling through beach-road WebSocket server functions correctly
- Transport trait abstraction allows switching transports and exposing WebRTC hooks

### Core Issue
The server's `start()` method (apps/beach/src/server/mod.rs) immediately:
1. Initializes the PTY with the command
2. Spawns readers for PTY output  
3. **Waits for the PTY command to complete** (lines 274-286)
4. Exits when the command finishes

For a command like `echo "Test"`, this happens in milliseconds, before any client can connect.

### Evidence
```rust
// apps/beach/src/server/mod.rs:274-286
// Wait for PTY task to complete
loop {
    let finished = {
        let guard = self.read_task.lock().unwrap();
        guard.as_ref().map(|t| t.is_finished()).unwrap_or(true)
    };
    if finished {
        stdin_task_stored.abort();
        break;  // <-- Server exits here
    }
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
}
```

Test results confirm this:
- Server with `echo "Test message"` terminates in ~100ms
- No WebRTC logs appear because transport is never used
- Client connection attempts fail with "session not found"

## Implementation Plan

### Phase 1: Enable Client Connections (Priority: Critical)

#### 1.1 Add Server Wait Mechanisms
**File**: `apps/beach/src/main.rs`
```rust
#[derive(Parser, Debug)]
struct Cli {
    // Add new flags
    #[arg(long, help = "Wait for at least one client before executing command")]
    wait_for_client: bool,
    
    #[arg(long, help = "Wait for WebRTC connection before executing command")]
    wait_for_webrtc: bool,
    
    #[arg(long, help = "Keep server alive after command exits")]
    keep_alive: bool,
    // ... existing fields
}
```

Pass these flags to the server (via TerminalServer::create) and expose a wrapper that waits before PTY start.

**File**: `apps/beach/src/server/mod.rs`
```rust
impl<T: Transport + Send + 'static> TerminalServer<T> {
    pub async fn wait_for_connections(&self, wait_for_webrtc: bool) {
        // NOTE: implement ServerSession::has_clients() and ServerSession::has_any_webrtc_connected()
        if wait_for_webrtc {
            while !self.session.read().await.has_any_webrtc_connected().await {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            debug_log!(self.debug_log, "Server: WebRTC connection established");
        } else {
            while !self.session.read().await.has_clients().await {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            debug_log!(self.debug_log, "Server: Client connected");
        }
    }
    
    pub async fn start_with_wait(&self, wait_for_client: bool, wait_for_webrtc: bool, keep_alive: bool) {
        if wait_for_client || wait_for_webrtc {
            self.wait_for_connections(wait_for_webrtc).await;
        }
        self.start().await;
        if keep_alive {
            loop { tokio::time::sleep(std::time::Duration::from_secs(1)).await; }
        }
    }
}
```

#### 1.2 Decouple PTY Execution from Server Lifecycle
**Rationale**: PTY should only start after clients connect, not immediately on server start.

**File**: `apps/beach/src/server/mod.rs`
```rust
pub async fn start(&self) {
    // If using start_with_wait(), waiting occurs before PTY init
    
    // THEN initialize and start PTY
    self.pty_manager.init(pty_size, cmd)
        .expect("Failed to initialize PTY");
    
    // Continue with existing PTY reader logic...
}
```

### Phase 2: Fix WebRTC Data Flow (Priority: High)

#### 2.1 Enforce Strict WebRTC Mode
**Issue**: Current code still falls back to WebSocket for app data in several paths.

Enforce in:
- ClientSession::send_to_server: if `BEACH_STRICT_WEBRTC=true` and no WebRTC data channel is connected, return an error; do not route AppMessage via WS.
- ServerSession::{send_to_client,broadcast_to_clients}: if strict and no per-client WebRTC transport is connected or send fails, return an error; do not route AppMessage via WS.

#### 2.2 Fix Message Router for WebRTC
**File**: `apps/beach/src/session/mod.rs`
```rust
async fn route_messages(mut receiver: mpsc::Receiver<Vec<u8>>, handler: Arc<dyn ServerMessageHandler>) {
    while let Some(data) = receiver.recv().await {
        // Deserialize and route to appropriate handler
        if let Ok(msg) = serde_json::from_slice::<AppMessage>(&data) {
            // Route based on message type
            match msg {
                AppMessage::TerminalOutput { .. } => {
                    handler.handle_terminal_output(msg).await;
                }
                AppMessage::TerminalInput { .. } => {
                    handler.handle_terminal_input(msg).await;
                }
                // ... other message types
            }
        }
    }
}
```

### Phase 3: Testing Infrastructure (Priority: High)

#### 3.1 In-Process WebRTC Echo Test
Create `tests/integration/webrtc_echo_local.rs` using `LocalSignalingChannel::create_pair()` and `WebRTCConfig::localhost(...)`.
Connect server (offerer) and client (answerer), then assert bidirectional send/recv over the data channel.

#### 3.2 End-to-End Test Script
**File**: `test-webrtc-e2e.sh`
```bash
#!/bin/bash
set -e

# Start beach-road
cargo run -p beach-road &
ROAD_PID=$!
sleep 2

# Start server with wait flag
BEACH_STRICT_WEBRTC=1 cargo run -p beach -- \
    --wait-for-webrtc \
    --debug-log /tmp/server.log \
    -- echo "WebRTC Works!" &
SERVER_PID=$!

# Extract session URL
sleep 1
SESSION_URL=$(grep "Session URL" /tmp/server.log | cut -d' ' -f3)

# Connect client
BEACH_STRICT_WEBRTC=1 cargo run -p beach -- \
    --join "$SESSION_URL" \
    --debug-log /tmp/client.log &
CLIENT_PID=$!

# Wait and check for WebRTC connection
sleep 5
if grep -q "WebRTC connection established" /tmp/server.log && \
   grep -q "Data channel.*opened" /tmp/client.log; then
    echo "✅ WebRTC connection successful!"
else
    echo "❌ WebRTC connection failed"
    exit 1
fi

# Cleanup
kill $ROAD_PID $SERVER_PID $CLIENT_PID 2>/dev/null || true
```

### Phase 4: Concurrency Hygiene (Priority: Medium)

#### 4.1 Fix RwLock-Guarded Awaits
**Issue**: Holding RwLock across await points can cause deadlocks.

**Current Antipattern**:
```rust
let session = self.session.read().await;
session.some_async_method().await;  // BAD: Holds lock across await
```

**Fix**:
```rust
let data = {
    let session = self.session.read().await;
    session.get_data_snapshot()  // Clone/extract needed data
};  // Lock released here
process_async(data).await;  // Safe to await without lock
```

**Files to audit**:
- `apps/beach/src/session/mod.rs`
- `apps/beach/src/server/mod.rs`
- `apps/beach/src/client/terminal_client.rs`

#### 4.2 Add Client-Side Timeouts
**File**: `apps/beach/src/client/terminal_client.rs`
```rust
pub async fn connect_with_timeout(&mut self, timeout_secs: u64) -> Result<()> {
    tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        self.establish_webrtc_connection()
    ).await
    .map_err(|_| anyhow!("WebRTC connection timeout after {}s", timeout_secs))?
}
```

### Phase 5: Dual-Channel Implementation (Priority: Future)

Once single-channel WebRTC works reliably:

1. **Create separate data channels**:
   - `beach/ctrl/1` - Control messages (input, resize)
   - `beach/term/1` - Terminal output

2. **Route messages by channel**:
   ```rust
   match data_channel.label() {
       "beach/ctrl/1" => handle_control_message(data).await,
       "beach/term/1" => handle_terminal_output(data).await,
       _ => log::warn!("Unknown channel: {}", data_channel.label()),
   }
   ```

3. **Implement flow control** on terminal output channel

## Testing Strategy

### Level 1: Unit Tests
- [ ] WebRTC transport creates offers/answers
- [ ] ICE candidates are gathered
- [ ] Data channels open/close properly

### Level 2: Integration Tests  
- [ ] In-process echo test (no network)
- [ ] Server waits for client connection
- [ ] Messages route through WebRTC when connected

### Level 3: End-to-End Tests
- [ ] Full server-client connection via beach-road
- [ ] Bidirectional data flow
- [ ] Proper cleanup on disconnect

## Success Criteria

1. **Immediate**: Server with `--wait-for-client` flag stays alive until client connects
2. **Short-term**: WebRTC data channel establishes and carries bidirectional traffic
3. **Medium-term**: All terminal I/O flows through WebRTC, no WebSocket fallback
4. **Long-term**: Dual-channel architecture with separate control/data streams

## Key Code Locations

### Server Start Logic
- `apps/beach/src/server/mod.rs:174-290` - Server::start() method
- **Problem**: Lines 274-286 wait for PTY completion instead of clients

### WebRTC Transport
- `apps/beach/src/transport/webrtc/mod.rs` - Main WebRTC implementation
- **Status**: Logging added, connection handling improved

### Session Management  
- `apps/beach/src/session/mod.rs` - Message routing
- **Issue**: Falls back to WebSocket silently

### Main Entry Point
- `apps/beach/src/main.rs:110-162` - Server initialization
- **Need**: Add wait flags and pass to server

## Environment Variables

- `BEACH_STRICT_WEBRTC=1` - Fail instead of WebSocket fallback
- Use `--debug-log /path/to/log` to enable file-based debug logging; combine with `-v/--verbose` or `BEACH_VERBOSE=1`
- (Optional) Use localhost-only WebRTC config in code (no STUN) for local tests
- `BEACH_VERBOSE=1` - Verbose output

## Next Steps

1. **Immediate** (Today):
   - Implement `--wait-for-client` flag
   - Add server connection waiting logic
   - Test with simple echo command

2. **Tomorrow**:
   - Fix message routing for WebRTC
   - Implement in-process echo test
   - Audit and fix RwLock patterns

3. **This Week**:
   - Complete end-to-end test suite
   - Implement strict WebRTC mode fully
   - Begin dual-channel design

## Appendix: ChatGPT Feedback Integration

The following recommendations from ChatGPT are reflected in the plan (some pending implementation):

1. 🔜 **In-process WebRTC echo test** - Phase 3.1
2. 🔜 **Server wait flags** - Phase 1.1
3. 🔜 **Strict WebRTC enforcement** - Phase 2.1
4. 🔜 **Client-side timeouts** - Phase 4.2
5. 🔜 **RwLock hygiene** - Phase 4.1
6. 🔜 **Remote handshake validation** - Phase 3.2
7. 🔜 **Dual-channel preparation** - Phase 5

## Conclusion

The primary blocker is architectural: the server doesn't wait for clients. Once fixed with the `--wait-for-client` mechanism, WebRTC can be properly tested and debugged. The implementation plan provides a clear path from the current broken state to a working dual-channel WebRTC system.

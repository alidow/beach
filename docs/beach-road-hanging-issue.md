# Beach-Road Server Hanging Issue

## Summary

The beach-road session server (`apps/beach-road`) experiences complete hangs that render it unresponsive to all HTTP and WebSocket requests. The hangs occur consistently after WebSocket connection errors, specifically "Connection reset without closing handshake" errors.

## Symptoms

1. **Complete Server Unresponsiveness**
   - Health endpoint (`/health`) times out
   - All HTTP routes become unreachable
   - Process appears running in systemd but doesn't respond
   - Requires full service restart to recover

2. **Triggering Pattern**
   - Occurs after WebSocket clients disconnect improperly
   - Error logged: `WebSocket error from peer <id>: WebSocket protocol error: Connection reset without closing handshake`
   - Server hangs within seconds to minutes after this error
   - Reproducible by having clients connect via WebSocket then disconnect abruptly

## Timeline of Investigation

### Initial Issue (Arc<Mutex<Storage>>)
**Problem**: `Arc<Mutex<Storage>>` pattern held Mutex lock across async `.await` points in Redis operations.

**Root Cause**: When Redis operations were slow, the Mutex would block all other requests since they had to serialize through a single lock.

**Fix Applied**:
- Changed from `Arc<Mutex<Storage>>` to `Arc<Storage>`
- Updated Storage methods from `&mut self` to `&self`
- Each method clones Redis `ConnectionManager` (which is designed to be cloned cheaply)
- No locks held across await points

**Files Modified**:
- `apps/beach-road/src/main.rs`: Removed Mutex wrapper
- `apps/beach-road/src/handlers.rs`: Changed SharedStorage type, removed `.lock().await`
- `apps/beach-road/src/storage.rs`: Changed all methods to `&self`, clone ConnectionManager per call
- `apps/beach-road/src/websocket.rs`: Removed `.lock().await` calls

**Result**: ✅ Server compiled and started successfully, but **still hung after WebSocket errors**

### Second Issue (DashMap Guards + RwLock)
**Problem**: Holding DashMap iteration guards while calling `.read().await` or `.write().await` on RwLock.

**Root Cause in `websocket.rs` Line 80** (original):
```rust
// PROBLEMATIC: Holding DashMap guards while awaiting
for session_entry in self.sessions.iter() {
    let session_id = session_entry.key().clone();
    let peers = session_entry.value();

    for peer_entry in peers.iter() {
        let peer_id = peer_entry.key().clone();
        let peer = peer_entry.value();

        let last_heartbeat = *peer.last_heartbeat.read().await; // BLOCKS HERE
        if last_heartbeat.elapsed() > timeout {
            stale_peers.push((session_id.clone(), peer_id.clone()));
        }
    }
}
```

**Root Cause in `websocket.rs` Line 580** (original):
```rust
// PROBLEMATIC: Holding DashMap guards while awaiting
if let Some(peers) = state.sessions.get(session_id) {
    if let Some(peer) = peers.get(peer_id) {
        *peer.last_heartbeat.write().await = std::time::Instant::now(); // BLOCKS HERE
    }
}
```

**Fix Applied**:
1. **Heartbeat Monitor** (lines 71-91): Collect `Arc<RwLock>` references first, then await outside the iteration
2. **Ping Handler** (lines 578-594): Clone the `Arc<RwLock>` before awaiting

**Result**: ✅ Code compiles and deploys, but **still hangs after WebSocket errors**

## Current State

**Status**: ❌ **Still Broken** - Server continues to hang despite both fixes

**Last Observed Hang**:
- Time: 2025-10-03 23:38:34 UTC
- Error: `WebSocket error from peer c3ae5ee8-4adf-4eab-93de-5d52cce254fc: WebSocket protocol error: Connection reset without closing handshake`
- Server hung shortly after this error
- Had to restart: `sudo systemctl restart beach-road`

**Evidence of Hanging**:
```bash
# From local machine
$ curl -s -m 5 https://api.beach.sh/health
Error  # Times out

# From server itself
$ ssh ec2-user@18.220.7.148 'curl -s -m 2 http://localhost:8080/health'
HUNG  # Local loopback also times out

# Service appears running
$ sudo systemctl status beach-road
● beach-road.service - Beach Road Service
     Active: active (running)
     # But doesn't respond to any requests
```

## Technical Architecture

### WebSocket Handler Flow

1. **Connection Establishment** (`websocket.rs:240-247`)
   ```rust
   pub async fn websocket_handler(
       ws: WebSocketUpgrade,
       Path(session_id): Path<String>,
       State(signaling): State<SignalingState>,
   ) -> Response {
       ws.on_upgrade(move |socket| handle_socket(socket, session_id, signaling, remote_addr))
   }
   ```

2. **Socket Handling** (`websocket.rs:250-275`)
   - Generates peer ID
   - Splits socket into sender/receiver
   - Creates unbounded channel for outbound messages
   - Spawns two tasks: message receiver loop and sender loop
   - Adds peer to session in DashMap

3. **Message Loop** (`websocket.rs:281-640`)
   - Iterates over incoming WebSocket messages
   - On error: logs and **breaks** the loop
   - Processes ClientMessage variants (Join, Signal, Ping, Debug)

4. **Error Path** (`websocket.rs:286-289`)
   ```rust
   let msg = match msg_result {
       Ok(m) => m,
       Err(e) => {
           error!("WebSocket error from peer {}: {}", peer_id, e);
           break;  // Exits the message loop
       }
   };
   ```

### Data Structures

```rust
// Global signaling state
pub struct SignalingState {
    sessions: Arc<DashMap<String, DashMap<String, PeerConnection>>>,
    storage: SharedStorage,  // Arc<Storage>
}

// Per-peer connection state
struct PeerConnection {
    peer_id: String,
    session_id: String,
    role: PeerRole,
    supported_transports: Vec<TransportType>,
    preferred_transport: Option<TransportType>,
    tx: mpsc::UnboundedSender<ServerMessage>,
    last_heartbeat: Arc<RwLock<std::time::Instant>>,
    label: Option<String>,
    remote_addr: Option<SocketAddr>,
}
```

### Heartbeat Monitor

Runs every 60 seconds, checks for peers with no heartbeat for 10 minutes:

```rust
async fn monitor_heartbeats(&self) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    let timeout = Duration::from_secs(600);

    loop {
        interval.tick().await;
        // Find stale connections (FIXED to not hold guards across await)
        // Remove stale peers
        // Broadcast PeerLeft messages
    }
}
```

## Hypotheses for Remaining Issue

### Hypothesis 1: Axum/Tokio Task Deadlock
**Theory**: When WebSocket error occurs, the spawned tasks might not be cleaning up properly, potentially blocking the Tokio runtime.

**Evidence**:
- Hang always occurs after WebSocket error
- Entire server becomes unresponsive (not just WebSocket routes)
- HTTP routes also time out

**To Investigate**:
- Check if `handle_socket` tasks are being properly dropped/cancelled
- Look for potential blocking operations in error cleanup
- Check if the sender/receiver task spawns are causing deadlocks

### Hypothesis 2: DashMap Internal Deadlock
**Theory**: Despite not holding guards across awaits, DashMap might have internal deadlocks with concurrent operations.

**Evidence**:
- Uses nested DashMaps: `DashMap<String, DashMap<String, PeerConnection>>`
- Multiple tasks accessing the same DashMap concurrently
- Removal operations during iteration

**To Investigate**:
- Check if `remove_peer` is safe to call while other iterations are happening
- Look for potential deadlock between outer and inner DashMap operations
- Consider replacing DashMap with `Arc<RwLock<HashMap>>` for debugging

### Hypothesis 3: Channel Deadlock
**Theory**: The `mpsc::unbounded_channel` used for peer communication might be blocking.

**Evidence**:
- Each peer has an unbounded sender/receiver pair
- Messages are sent via `tx.send()` which could block if receiver is gone
- Error path breaks the loop but sender might still be referenced elsewhere

**To Investigate**:
- Check if `tx.send()` can block even with unbounded channel
- Look for places where sender is cloned and might outlive the receiver
- Ensure proper cleanup when peer disconnects

### Hypothesis 4: Redis Connection Pool Exhaustion
**Theory**: Despite cloning ConnectionManager, the underlying pool might be exhausted.

**Evidence**:
- Multiple concurrent operations cloning ConnectionManager
- Redis operations in hot paths (every heartbeat, every message)
- No explicit connection limit or pool size configuration

**To Investigate**:
- Check Redis `ConnectionManager` pool configuration
- Look for leaked connections not being returned to pool
- Monitor actual Redis connection count during hang

### Hypothesis 5: Axum Response/Stream Not Finishing
**Theory**: WebSocket upgrade or HTTP response handlers might not be completing properly.

**Evidence**:
- Entire HTTP server becomes unresponsive
- Not just new connections, but all routes
- Suggests Axum's request handling is stuck

**To Investigate**:
- Check if `ws.on_upgrade()` is properly handling errors
- Look for uncompleted Futures in the upgrade path
- Check if HTTP handlers are being properly released

## Debugging Steps to Take

### 1. Add Extensive Logging
```rust
// In websocket.rs handle_socket
debug!("Starting handle_socket for peer {} session {}", peer_id, session_id);

// Before and after every await
debug!("About to acquire DashMap guard");
// ... operation
debug!("Released DashMap guard");

// In error path
error!("WebSocket error, starting cleanup for peer {}", peer_id);
// ... cleanup
error!("Cleanup complete for peer {}", peer_id);
```

### 2. Add Tokio Console Support
Enable tokio-console to monitor task states:
```toml
[dependencies]
console-subscriber = "0.2"
tokio = { version = "1", features = ["full", "tracing"] }
```

### 3. Reproduce Locally with Controlled Disconnect
Create a test client that:
1. Connects via WebSocket
2. Sends a message
3. Forcefully closes connection without proper handshake
4. Monitor server for hang

### 4. Simplify to Isolate Issue
Create minimal reproduction:
- Remove all Redis operations (use in-memory HashMap)
- Remove heartbeat monitoring
- Simple echo server with forced disconnects
- See if hang still occurs

### 5. Add Timeout Guards
Wrap all async operations in timeouts:
```rust
tokio::time::timeout(Duration::from_secs(5), async_operation)
    .await
    .map_err(|_| "operation timed out")?
```

### 6. Replace DashMap Temporarily
```rust
// Replace
sessions: Arc<DashMap<String, DashMap<String, PeerConnection>>>

// With
sessions: Arc<RwLock<HashMap<String, HashMap<String, PeerConnection>>>>
```

This will be slower but easier to reason about and debug.

## Files to Examine Closely

1. **`apps/beach-road/src/websocket.rs`**
   - Lines 250-641: `handle_socket` function
   - Lines 62-110: `monitor_heartbeats` function
   - Lines 240-247: `websocket_handler` entry point

2. **`apps/beach-road/src/storage.rs`**
   - All async methods using ConnectionManager
   - Connection pool behavior

3. **`apps/beach-road/src/main.rs`**
   - Axum router setup (lines 79-103)
   - Tokio runtime configuration

## Environment Details

- **OS**: Amazon Linux 2023 on EC2 (us-east-2)
- **Rust Version**: 1.92.0-nightly
- **Key Dependencies**:
  - `axum = "0.7"`
  - `tokio = { version = "1", features = ["full"] }`
  - `dashmap = "6"`
  - `redis = { version = "0.27", features = ["tokio-comp", "connection-manager"] }`

## Reproduction Steps

1. Deploy beach-road to server
2. Have a beach client connect via WebSocket:
   ```bash
   cargo run --bin beach -- host --session-server https://api.beach.sh -- bash
   ```
3. Forcefully kill the client (Ctrl+C or SIGKILL)
4. Within seconds to minutes, observe server hang
5. Verify hang:
   ```bash
   curl -m 5 https://api.beach.sh/health  # Times out
   ```

## Related Issues

- `beach ssh` command hangs because it cannot reach api.beach.sh when beach-road is hung
- Singapore test instance cannot connect to beach-road when it's in hung state
- All beach operations requiring session server are blocked

## Next Steps

1. ⚠️ **URGENT**: Identify why server hangs after WebSocket errors
2. Add comprehensive error handling and cleanup in WebSocket error path
3. Add graceful shutdown handling for peer disconnects
4. Consider circuit breaker pattern for Redis operations
5. Add health check that actually tests request handling (not just TCP connection)
6. Implement proper monitoring/alerting for server hangs

## Questions for Further Investigation

1. Is the Tokio runtime itself blocking, or just the Axum handlers?
2. Are there any blocking operations (file I/O, synchronous network calls) in async code?
3. Is `DashMap` actually async-safe in our usage pattern?
4. Does the Redis `ConnectionManager` have a maximum pool size we're hitting?
5. Are WebSocket upgrade tasks being properly awaited/joined?
6. Is there a memory leak causing resource exhaustion?
7. Could this be a Tokio executor starvation issue?

## Temporary Workaround

**Manual restart when hung**:
```bash
ssh -i ~/.ssh/beach-road-deploy.pem ec2-user@18.220.7.148 'sudo systemctl restart beach-road'
```

**Health check script** (to detect and auto-restart):
```bash
#!/bin/bash
if ! curl -s -m 5 http://localhost:8080/health > /dev/null; then
    echo "Beach-road hung, restarting..."
    sudo systemctl restart beach-road
fi
```

Add to cron: `* * * * * /path/to/health-check.sh`

# Dual-Channel WebRTC Implementation Design

Status: Next Up (High Priority)  
Date: 2025-09-08  
Author: Beach Development Team  

## Executive Summary

This document outlines the implementation plan for integrating WebRTC transport into Beach, establishing peer-to-peer connections for all application traffic while using beach-road purely as a signaling server for connection establishment. The implementation includes clear testable milestones where bi-directional P2P communication can be verified at each phase.

## Current State Analysis

### What's Working Now
- WebSocket connection to beach-road for signaling and message relay
- Minimal client and server exchange app messages via `ClientMessage::Signal` routed by beach-road
- Channel abstraction layer implemented (`apps/beach/src/transport/channel.rs`)
- WebRTC transport skeleton with chunking support exists (local/in-memory signaling only)
- Message routing infrastructure in place

### What's Missing
- WebRTC signaling integration through beach-road (remote signaling adapter)
- Actual WebRTC peer connection establishment via remote signaling
- SDP offer/answer exchange via beach-road
- ICE candidate gathering and exchange via beach-road
- Dual-channel message routing (create/map `beach/ctrl/1` + `beach/term/1`)
- Frame versioning and resync protocol (planned later phase)

## Architecture Overview

```
Connection Establishment Phase:
┌─────────────────┐                    ┌──────────────┐                    ┌─────────────────┐
│  Beach Client   │                    │  Beach-Road  │                    │  Beach Server   │
│                 │◄──── WebSocket ───►│  (Signaling  │◄─── WebSocket ────►│                 │
│                 │   Join + SDP/ICE   │    Server)   │   Join + SDP/ICE   │                 │
└─────────────────┘                    └──────────────┘                    └─────────────────┘

After WebRTC Connection Established (P2P):
┌─────────────────┐                                                        ┌─────────────────┐
│  Beach Client   │                                                        │  Beach Server   │
│                 │◄════════════════ WebRTC P2P Connection ══════════════►│                 │
│                 │     Control Channel (reliable, ordered)                │                 │
│                 │     Output Channel (unreliable, unordered)             │                 │
│                 │     All terminal I/O flows P2P                         │                 │
└─────────────────┘                                                        └─────────────────┘
                           Beach-road no longer sees app traffic
```

### Key Architecture Points

1. **Beach-road is ONLY a signaling server**:
   - Facilitates WebRTC connection establishment (SDP/ICE exchange)
   - Never sees application data after P2P connection
   - Never sees passphrases (removed in Phase 0)

2. **All application traffic is P2P**:
   - Once WebRTC connects, terminal I/O flows directly between peers
   - No relay through beach-road for app messages
   - DTLS encryption by default on WebRTC data channels

3. **Security handshake happens over P2P** (future phase):
   - Passphrase-based authentication occurs AFTER P2P establishment
   - Uses the secure P2P channel, not beach-road
   - Additional app-level encryption if needed

## Implementation Phases with Testable Milestones

### Phase 1: WebRTC Connection Establishment (ASAP)

#### Goal
Use beach-road as a signaling server to establish P2P WebRTC connections between beach peers.

#### Implementation Steps

1. **Define typed WebRTC signals; keep wire compatible**
   - Keep `ClientMessage::Signal { signal: serde_json::Value }` on the wire for backward compatibility with beach-road.
   - Add a typed enum in Beach client for WebRTC payloads that mirrors beach-road’s shape (`apps/beach-road/src/signaling.rs::TransportSignal::WebRTC{ signal: WebRTCSignal }`).
   - Provide helpers to convert to/from `serde_json::Value`:
     - `fn webrtc_signal_to_value(WebRTCSignal) -> serde_json::Value`
     - `fn value_to_webrtc_signal(v: &serde_json::Value) -> Option<WebRTCSignal>`
   - This allows typed handling locally without changing the server’s relay contract.

2. **Create Remote Signaling Module** (`apps/beach/src/transport/webrtc/remote_signaling.rs`)
```rust
use crate::protocol::signaling::{ClientMessage, ServerMessage, TransportSignal};
use crate::session::signaling_transport::SignalingTransport;

pub struct RemoteSignalingChannel {
    signaling: Arc<SignalingTransport<WebSocketTransport>>,
    peer_id: String,
    remote_peer_id: Arc<RwLock<Option<String>>>,
}

impl RemoteSignalingChannel {
    pub async fn send_offer(&self, sdp: String) -> Result<()> {
        let signal = TransportSignal::Offer { sdp };
        self.send_signal(signal).await
    }
    
    pub async fn send_answer(&self, sdp: String) -> Result<()> {
        let signal = TransportSignal::Answer { sdp };
        self.send_signal(signal).await
    }
    
    pub async fn send_ice_candidate(&self, candidate: RTCIceCandidate) -> Result<()> {
        let signal = TransportSignal::IceCandidate {
            candidate: candidate.to_json()?.candidate,
            sdp_mid: candidate.to_json()?.sdp_mid,
            sdp_mline_index: candidate.to_json()?.sdp_mline_index,
        };
        self.send_signal(signal).await
    }
    
    async fn send_signal(&self, signal: TransportSignal) -> Result<()> {
        if let Some(to_peer) = self.remote_peer_id.read().await.as_ref() {
            self.signaling.send(ClientMessage::Signal {
                to_peer: to_peer.clone(),
                signal,
            }).await?;
        }
        Ok(())
    }
}
```

3. **Integrate with WebRTCTransport and Session** (`transport/webrtc/mod.rs`, `session/mod.rs`)
```rust
impl<T: Transport> ClientSession<T> {
    pub async fn start_webrtc_connection(&mut self) -> Result<()> {
        // Wait for server peer ID from JoinSuccess
        let server_peer_id = self.wait_for_server_peer().await?;
        
        // Create WebRTC transport with remote signaling
        let webrtc = WebRTCTransport::new_with_signaling(
            self.signaling_transport.clone(),
            self.id.clone(),
            server_peer_id,
            TransportMode::Client,
        ).await?;
        
        // Initiate connection as client
        webrtc.connect_as_client().await?;
        
        // Replace transport
        self.session.transport = Arc::new(webrtc);
        Ok(())
    }
}
```

#### TESTABLE MILESTONE 1
```bash
# Terminal 1: Start beach-road with debug logging
RUST_LOG=debug cargo run -p beach-road

# Terminal 2: Start server with verbose logging
BEACH_VERBOSE=1 cargo run -p beach -- --passphrase test123 bash

# Terminal 3: Join as client with verbose logging
BEACH_VERBOSE=1 cargo run -p beach -- --join localhost:8080/<session-id> --passphrase test123

# Expected Output:
# - "Sending Signal: Offer" in server logs
# - "Received Signal: Offer" in client logs
# - "Sending Signal: Answer" in client logs
# - "Received Signal: Answer" in server logs
# - "Sending Signal: IceCandidate" in both logs
# - "WebRTC: ICE connection state: Connected" in both logs

# Test: Type "echo hello" in client
# Verify: Command executes on server, output appears in client
```

### Phase 2: Single Channel WebRTC (Week 2)

#### Goal
Replace WebSocket data transport with a single reliable WebRTC data channel.

#### Implementation Steps

1. **Update WebRTC Transport** (`transport/webrtc/mod.rs`)
```rust
impl WebRTCTransport {
    pub async fn connect_with_remote_signaling(
        mut self,
        signaling: Arc<RemoteSignalingChannel>,
        is_offerer: bool,
    ) -> Result<Self> {
        // Setup ICE candidate handler
        self.peer_connection.on_ice_candidate(Box::new(move |candidate| {
            let signaling = signaling.clone();
            Box::pin(async move {
                if let Some(candidate) = candidate {
                    signaling.send_ice_candidate(candidate).await;
                }
            })
        }));
        
        if is_offerer {
            // Server creates offer
            let offer = self.peer_connection.create_offer(None).await?;
            self.peer_connection.set_local_description(offer.clone()).await?;
            signaling.send_offer(offer.sdp).await?;
            
            // Wait for answer
            let answer = signaling.wait_for_answer().await?;
            self.peer_connection.set_remote_description(answer).await?;
        } else {
            // Client waits for offer
            let offer = signaling.wait_for_offer().await?;
            self.peer_connection.set_remote_description(offer).await?;
            
            // Create and send answer
            let answer = self.peer_connection.create_answer(None).await?;
            self.peer_connection.set_local_description(answer.clone()).await?;
            signaling.send_answer(answer.sdp).await?;
        }
        
        // Wait for connection
        self.wait_for_connection().await?;
        Ok(self)
    }
}
```

2. **Route Application Messages Through WebRTC** (`session/message_handlers/mod.rs`)
```rust
impl MessageRouter {
    pub async fn route_to_transport(&mut self, msg: AppMessage) -> Result<()> {
        // Check if WebRTC is connected
        if self.webrtc_transport.is_some() {
            // Route through WebRTC data channel
            let data = bincode::serialize(&msg)?;
            self.webrtc_transport.send(&data).await?;
        } else {
            // Fallback to WebSocket
            self.websocket_transport.send(&msg).await?;
        }
        Ok(())
    }
}
```

#### TESTABLE MILESTONE 2
```bash
# Same setup as Milestone 1

# Test 1: Basic I/O
# Type in client: ls -la
# Verify: Directory listing appears in client

# Test 2: Interactive application
# Type in client: vim test.txt
# Verify: Vim opens, can edit and save

# Test 3: Real-time output
# Type in client: top
# Verify: Live process updates visible

# Check logs for:
# - "WebRTC data channel opened"
# - "Sending via WebRTC: X bytes"
# - "Received via WebRTC: X bytes"

# Verify WebSocket is only used for signaling:
# - No "TerminalInput" or "TerminalOutput" in WebSocket logs
```

### Phase 3: Dual-Channel Implementation (Week 3)

#### Goal
Implement separate control and output channels with appropriate reliability settings.

#### Implementation Steps

1. **Create Dual Channels** (`transport/webrtc/mod.rs`)
```rust
impl WebRTCTransport {
    async fn create_dual_channels(&self) -> Result<()> {
        // Create control channel (reliable, ordered)
        let control_channel = self.create_channel_internal(
            ChannelPurpose::Control
        ).await?;
        
        // Create output channel (unreliable, unordered)
        let output_channel = self.create_channel_internal(
            ChannelPurpose::Output
        ).await?;
        
        // Store channels
        let mut channels = self.channels.write().await;
        channels.insert(ChannelPurpose::Control, control_channel);
        channels.insert(ChannelPurpose::Output, output_channel);
        
        Ok(())
    }
}
```

2. **Implement Message Routing** (`session/message_handlers/channel_router.rs`)
```rust
pub struct ChannelRouter {
    control_channel: Arc<dyn TransportChannel>,
    output_channel: Arc<dyn TransportChannel>,
}

impl ChannelRouter {
    pub async fn route_message(&self, msg: &AppMessage) -> Result<()> {
        match msg {
            // Control channel messages (must be reliable)
            AppMessage::TerminalInput { .. } |
            AppMessage::TerminalResize { .. } |
            AppMessage::Protocol { .. } => {
                if std::env::var("BEACH_VERBOSE").is_ok() {
                    eprintln!("Routing to control channel: {:?}", msg);
                }
                let data = bincode::serialize(msg)?;
                self.control_channel.send(&data).await?;
            },
            
            // Output channel messages (can be lossy)
            AppMessage::TerminalOutput { .. } => {
                if std::env::var("BEACH_VERBOSE").is_ok() {
                    eprintln!("Routing to output channel: {:?}", msg);
                }
                let data = bincode::serialize(msg)?;
                self.output_channel.send(&data).await?;
            },
            
            _ => {
                // Default to control channel
                let data = bincode::serialize(msg)?;
                self.control_channel.send(&data).await?;
            }
        }
        Ok(())
    }
}
```

#### TESTABLE MILESTONE 3
```bash
# Start with verbose logging and channel debugging
BEACH_VERBOSE=1 BEACH_CHANNEL_DEBUG=1 cargo run -p beach -- --passphrase test123 bash
BEACH_VERBOSE=1 BEACH_CHANNEL_DEBUG=1 cargo run -p beach -- --join localhost:8080/<id> --passphrase test123

# Test 1: Verify channel separation
# Type in client: echo "test"
# Expected logs:
# - "Routing to control channel: TerminalInput"
# - "Routing to output channel: TerminalOutput"

# Test 2: High-throughput output
# Type in client: seq 1 10000
# Verify:
# - Output flows smoothly
# - Log shows "Output channel: sent X messages"

# Test 3: Interactive with mixed I/O
# Type in client: python3
# >>> for i in range(100): print(f"Line {i}")
# Verify:
# - Input goes through control channel
# - Output goes through output channel
# - No message loss for control, some acceptable loss for output

# Test 4: Channel statistics
# Add --channel-stats flag to see:
# - Control channel: X bytes sent, Y bytes received, 0 dropped
# - Output channel: X bytes sent, Y bytes received, Z dropped (acceptable)
```

### Phase 4: Frame Versioning and Resync (Week 4)

#### Goal
Add resilience through frame versioning and automatic resynchronization.

#### Implementation Steps

1. **Add Versioning to Messages** (`protocol/mod.rs`)
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionedOutput {
    pub version: u64,
    pub base_version: Option<u64>,  // For deltas
    pub data: Vec<u8>,
    pub is_snapshot: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlMessage {
    Ack { version: u64 },
    ResyncRequest { last_version: u64 },
    // ... other control messages
}
```

2. **Implement Frame Tracker** (`server/terminal_state/frame_tracker.rs`)
```rust
pub struct FrameTracker {
    current_version: AtomicU64,
    delta_window: Arc<RwLock<VecDeque<VersionedOutput>>>,
    window_size: Duration,
    last_snapshot: Arc<RwLock<Option<VersionedOutput>>>,
}

impl FrameTracker {
    pub fn create_output(&self, data: Vec<u8>, client_ack: u64) -> VersionedOutput {
        let version = self.current_version.fetch_add(1, Ordering::SeqCst);
        
        // Determine if we need a snapshot
        if self.should_send_snapshot(client_ack) {
            VersionedOutput {
                version,
                base_version: None,
                data,
                is_snapshot: true,
            }
        } else {
            VersionedOutput {
                version,
                base_version: Some(client_ack),
                data,
                is_snapshot: false,
            }
        }
    }
    
    pub async fn handle_resync_request(&self, last_version: u64) -> VersionedOutput {
        // Try to build delta chain
        if let Some(chain) = self.build_delta_chain(last_version).await {
            return chain;
        }
        
        // Fall back to snapshot
        self.create_snapshot().await
    }
}
```

3. **Client-side Version Tracking** (`client/frame_receiver.rs`)
```rust
pub struct FrameReceiver {
    last_applied_version: u64,
    pending_frames: BTreeMap<u64, VersionedOutput>,
    ack_interval: Duration,
    last_ack_time: Instant,
}

impl FrameReceiver {
    pub async fn handle_frame(&mut self, frame: VersionedOutput) -> Result<()> {
        if frame.is_snapshot {
            // Apply snapshot immediately
            self.apply_snapshot(frame)?;
            self.last_applied_version = frame.version;
            self.pending_frames.clear();
        } else if let Some(base) = frame.base_version {
            if base == self.last_applied_version {
                // Can apply delta
                self.apply_delta(frame)?;
                self.last_applied_version = frame.version;
                
                // Try to apply pending frames
                self.apply_pending_frames()?;
            } else if base > self.last_applied_version {
                // Gap detected, request resync
                self.request_resync().await?;
            } else {
                // Out of order, store for later
                self.pending_frames.insert(frame.version, frame);
            }
        }
        
        // Send ack if needed
        if self.last_ack_time.elapsed() > self.ack_interval {
            self.send_ack(self.last_applied_version).await?;
            self.last_ack_time = Instant::now();
        }
        
        Ok(())
    }
}
```

#### TESTABLE MILESTONE 4
```bash
# Test with simulated packet loss (Linux/macOS)
# Terminal 1: Add packet loss to loopback
sudo tc qdisc add dev lo root netem loss 5%

# Terminal 2: Start server
BEACH_VERBOSE=1 cargo run -p beach -- --passphrase test123 bash

# Terminal 3: Start client
BEACH_VERBOSE=1 cargo run -p beach -- --join localhost:8080/<id> --passphrase test123

# Test 1: Continuous output with loss
# In client type: while true; do date; sleep 0.1; done
# Expected logs:
# - "Gap detected: expected version X, got Y"
# - "Sending ResyncRequest"
# - "Received snapshot, version Z"
# - Output continues despite packet loss

# Test 2: Large output burst
# In client type: cat /var/log/system.log
# Verify:
# - Some frames dropped (logged)
# - Automatic recovery via resync
# - Complete file displayed

# Test 3: Interactive under loss
# In client type: vim large_file.txt
# Verify:
# - Editing remains responsive
# - Screen updates may skip but recover

# Clean up packet loss simulation
sudo tc qdisc del dev lo root

# Test 4: Metrics
# Check --metrics flag output:
# - Frames sent: X
# - Frames acknowledged: Y
# - Resyncs requested: Z
# - Snapshots sent: W
```

### Phase 5: Performance Optimization (Week 5)

#### Goal
Optimize for production use with adaptive strategies and comprehensive metrics.

#### Implementation Steps

1. **Adaptive Channel Selection** (`session/adaptive_router.rs`)
```rust
pub struct AdaptiveRouter {
    metrics: Arc<RwLock<ChannelMetrics>>,
    fallback_threshold: f64,  // Loss rate to trigger fallback
}

impl AdaptiveRouter {
    pub async fn route_output(&self, data: Vec<u8>) -> Result<()> {
        let metrics = self.metrics.read().await;
        
        if metrics.output_channel_loss_rate() > self.fallback_threshold {
            // High loss, use reliable control channel
            self.control_channel.send(&data).await?;
        } else {
            // Normal operation, use unreliable output channel
            self.output_channel.send(&data).await?;
        }
        Ok(())
    }
}
```

2. **Implement Metrics Collection** (`transport/webrtc/metrics.rs`)
```rust
#[derive(Debug, Clone)]
pub struct WebRTCMetrics {
    pub bytes_sent: AtomicU64,
    pub bytes_received: AtomicU64,
    pub packets_sent: AtomicU64,
    pub packets_lost: AtomicU64,
    pub rtt_ms: AtomicU64,
    pub jitter_ms: AtomicU64,
}

impl WebRTCMetrics {
    pub async fn collect_stats(&self, pc: &RTCPeerConnection) {
        let stats = pc.get_stats().await;
        // Parse WebRTC stats and update metrics
    }
    
    pub fn report(&self) -> MetricsReport {
        MetricsReport {
            throughput_mbps: self.calculate_throughput(),
            loss_rate: self.calculate_loss_rate(),
            rtt_ms: self.rtt_ms.load(Ordering::Relaxed),
            health_score: self.calculate_health_score(),
        }
    }
}
```

#### TESTABLE MILESTONE 5
```bash
# Terminal 1: Start server with metrics
cargo run -p beach -- --passphrase test123 --metrics-interval 1s bash

# Terminal 2: Start client with metrics
cargo run -p beach -- --join localhost:8080/<id> --passphrase test123 --metrics-interval 1s

# Test 1: Baseline performance
# In client: time (seq 1 100000)
# Record: Execution time, CPU usage, memory usage

# Test 2: High-throughput test
# In client: cat /dev/urandom | base64 | head -n 10000
# Monitor metrics output:
# - Throughput: X Mbps
# - Loss rate: Y%
# - RTT: Z ms

# Test 3: Comparison with WebSocket
# Run same tests with --force-websocket flag
# Compare:
# - Latency (should be lower with WebRTC)
# - Throughput (should be higher with WebRTC)
# - CPU usage (should be comparable)

# Test 4: Adaptive routing
# Simulate degraded network:
sudo tc qdisc add dev lo root netem loss 10% delay 100ms

# In client: Run intensive output command
# Verify in logs:
# - "High loss detected, falling back to control channel"
# - Output continues reliably

# Test 5: Long-running stability
# Run for 10 minutes with continuous I/O
# Monitor for:
# - Memory leaks (stable memory usage)
# - Connection stability (no drops)
# - Consistent performance
```

## File Structure and Changes

### New Files to Create
```
apps/beach/src/transport/webrtc/
├── remote_signaling.rs     # WebSocket-based signaling for WebRTC
├── metrics.rs               # Performance metrics collection
└── adaptive.rs              # Adaptive routing strategies

apps/beach/src/session/
├── channel_router.rs        # Dual-channel message routing
└── frame_tracker.rs         # Frame versioning and resync

apps/beach/src/client/
└── frame_receiver.rs        # Client-side frame management
```

### Files to Modify
```
apps/beach/src/
├── transport/webrtc/mod.rs  # Add remote signaling support
├── session/mod.rs            # Integrate WebRTC flow
├── protocol/signaling/mod.rs # Add WebRTC signal types
└── main.rs                   # Add CLI flags for testing

apps/beach-road/src/
├── websocket.rs              # Forward WebRTC signals
└── main.rs                   # Add WebRTC signal handling
```

## Testing Strategy

### Unit Tests
```rust
// tests/webrtc_signaling_test.rs
#[tokio::test]
async fn test_offer_answer_exchange() { }

#[tokio::test]
async fn test_ice_candidate_exchange() { }

// tests/dual_channel_test.rs
#[tokio::test]
async fn test_channel_creation() { }

#[tokio::test]
async fn test_message_routing() { }

// tests/frame_versioning_test.rs
#[tokio::test]
async fn test_frame_sequencing() { }

#[tokio::test]
async fn test_resync_protocol() { }
```

### Integration Tests
```rust
// tests/integration/webrtc_e2e_test.rs
#[tokio::test]
async fn test_full_connection_flow() { }

#[tokio::test]
async fn test_dual_channel_communication() { }

#[tokio::test]
async fn test_resilience_under_loss() { }
```

### Performance Benchmarks
```rust
// benches/webrtc_bench.rs
fn bench_single_channel_throughput(b: &mut Bencher) { }
fn bench_dual_channel_throughput(b: &mut Bencher) { }
fn bench_latency_comparison(b: &mut Bencher) { }
```

## Debug and Monitoring Tools

### CLI Flags
```bash
--webrtc-stats          # (optional) Show connection statistics in logs
--channel-info          # (optional) Display channel states and metrics
--force-websocket       # Disable WebRTC for comparison
--force-single-channel  # Use only control channel
--metrics-interval <s>  # Set metrics reporting interval
--simulate-loss <pct>   # Simulate packet loss for testing
```

### Debug Endpoints (beach-road)
- Not yet implemented. Use logs for verification in the short term.
- Consider adding `/debug/sessions` and (optionally) a lightweight WebRTC stats view as a follow-up.

## Security Considerations

### Phase 1-3 Security
- Passphrase still used for session authentication
- WebRTC connection authenticated via beach-road session
- DTLS encryption for data channels

### Future Security Enhancements (Phase 6+)
- Remove passphrase from signaling path
- Implement sealed signaling with ChaCha20-Poly1305
- Add application-level handshake with Noise protocol
- Channel binding to prevent MITM attacks

## Migration Path

### Backward Compatibility
1. WebRTC is opt-in initially (feature flag)
2. Fallback to WebSocket if WebRTC fails
3. Single-channel mode for compatibility
4. Gradual rollout with metrics monitoring

### Rollout Strategy
1. Internal testing with feature flag
2. Beta users with opt-in flag
3. Gradual percentage rollout
4. Full deployment with WebSocket fallback
5. Deprecate WebSocket transport

## Success Metrics

### Performance Targets
- Latency: 30% reduction vs WebSocket
- Throughput: 50% increase for large outputs
- Packet loss tolerance: 5% without user impact
- Connection establishment: < 2 seconds

### Quality Metrics
- Connection success rate: > 99%
- Mean time between failures: > 24 hours
- Recovery time from disconnect: < 5 seconds
- User-perceived responsiveness: < 50ms input latency

## Timeline

- ASAP: Phase 1 – Remote signaling integration
- Next: Phase 2 – Single/dual channel data paths
- Then: Phase 3 – Dual-channel stabilization
- Later: Phase 4+ – Versioning, resync, and optimization

## Sanity Checks

- Wire compatibility: keep `ClientMessage::Signal.signal: serde_json::Value` and wrap typed signals under the existing JSON shape (no server change required).
- DataChannel constraints: set only one of `max_retransmits` or `max_packet_lifetime` for unreliable channels; set `ordered=false` for the output channel.
- Channel mapping: ensure `on_data_channel` maps incoming labels to `ChannelPurpose` so receivers register channels correctly.
- Fallbacks: introduce `--force-websocket` and `--force-single-channel` flags (and env vars) for controlled rollout.
- Chunking: current 60KB chunk size is safe under typical 64KB limits; continue reassembly with back-pressure.

## Conclusion

This implementation plan provides a clear path to WebRTC integration with testable milestones at each phase. The dual-channel architecture will significantly improve performance while maintaining reliability. The phased approach allows for continuous testing and validation, ensuring a smooth transition from WebSocket to WebRTC transport.

# Dual-Channel WebRTC Implementation Plan

Status: Draft v1  
Date: 2024-01-09  
Author: Implementation Team  

## CRITICAL: Developer Experience & Modes

We optimize for a dead‑simple, “just works” CLI flow while keeping strong security. Two modes:

- Public Beach Mode
  - Default when no credentials/profile/env vars are set (AWS‑CLI style). No sign‑in needed.
  - Start a server: `beach [-- cmd ...] [--passphrase|--pp <code>]`
    - If no passphrase provided, CLI generates a short, human‑friendly code and shows an interstitial (press Enter or wait 60s). The code is ephemeral and rate‑limited.
    - Session URLs are issued under `public.<session-host>/...` to make the trust context obvious.
  - Join: `beach --join <url|id> [--passphrase <code>]` (or prompt if missing). Also supports `BEACH_PASSPHRASE`.
  - Security: sealed setup messages using the passphrase, then a short post‑connect handshake on the reliable control channel; the passphrase is never sent in plaintext.

- Private Beach Mode
  - Users authenticate via Clerk and work with profiles like AWS CLI.
  - `beach login` opens a browser or device code flow, writes `~/.beach/credentials` and `~/.beach/config` with profiles.
  - Start a server requires auth: `beach [--profile <name>] [-- cmd ...]`. Joins also require auth.
  - Sessions are issued under `private.<session-host>/...`. The beach server enforces authorization (group membership, policies).
  - Passphrase optional as an extra gate; Clerk auth is mandatory in private mode.

Profiles & env vars (mode selection):
- Files: `~/.beach/config`, `~/.beach/credentials` (default profile like AWS).
- Env: `BEACH_PROFILE`, `BEACH_SESSION_SERVER`, `BEACH_PASSPHRASE`, Clerk envs (`CLERK_JWKS_URL`, etc.).
- If no profile/credentials resolved → Public mode; otherwise Private. `--mode private|public` can override.

## Executive Summary

This document outlines the implementation plan for evolving Beach's transport layer to support dual-channel WebRTC with zero-trust security. The plan addresses critical security vulnerabilities in the current architecture and prepares the system for resilient, low-latency terminal streaming.

## Problem Statement

### Current Architecture Limitations

1. **Security Vulnerability**: Passphrase is transmitted to beach-road (untrusted signaling server), violating zero-trust principles
2. **Single Channel Bottleneck**: All traffic flows through one reliable channel, causing head-of-line blocking
3. **No Message Prioritization**: Control messages compete with bulk output data
4. **Missing Resync Capability**: No versioning or recovery mechanism for lost frames
5. **Plaintext Signaling**: SDP/ICE candidates exposed to MITM attacks
6. **No Authentication**: Direct PTY access without verifying peer identity

### Goals

- **Immediate**: Remove passphrase from signaling path
- **Short-term**: Enable dual-channel architecture (control + output)
- **Medium-term**: Add frame versioning and resync
- **Long-term**: Full zero-trust with sealed signaling and authenticated handshake

## Architecture Overview

```
┌─────────────┐                          ┌─────────────┐
│   Client    │                          │   Server    │
├─────────────┤                          ├─────────────┤
│  Transport  │◄──── Control Channel ───►│  Transport  │
│   Layer     │      (reliable)          │   Layer     │
│             │                          │             │
│             │◄──── Output Channel ────►│             │
│             │      (unreliable)        │             │
└─────────────┘                          └─────────────┘
      ▲                                         ▲
      │                                         │
      └──────── Sealed Signaling ──────────────┘
            (via untrusted beach-road)
```

## Implementation Phases

### Phase 1: Transport Layer Evolution

#### 1.1 Channel Abstraction

**File**: `apps/beach/src/transport/channel.rs` (new)

```rust
/// Reliability configuration for a transport channel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelReliability {
    /// Reliable, ordered delivery (TCP-like)
    Reliable,
    /// Unreliable, unordered delivery (UDP-like)
    Unreliable {
        max_retransmits: Option<u16>,
        max_packet_lifetime: Option<u16>,
    },
}

/// Purpose of a channel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelPurpose {
    Control,    // Handshake, auth, input, acks, resync, control
    Output,     // Terminal frames (lossy allowed)
    Custom(u8), // Future extension
}

/// Individual channel within a transport
#[async_trait]
pub trait TransportChannel: Send + Sync {
    fn label(&self) -> &str;
    fn reliability(&self) -> ChannelReliability;
    fn purpose(&self) -> ChannelPurpose;
    async fn send(&self, data: &[u8]) -> Result<()>;
    async fn recv(&mut self) -> Option<Vec<u8>>;
    fn is_open(&self) -> bool;
}
```

#### 1.2 Updated Transport Trait

**File**: `apps/beach/src/transport/mod.rs` (modify)

```rust
/// Transport with multi-channel support
#[async_trait]
pub trait Transport: Send + Sync {
    /// Get or create a channel by purpose
    async fn channel(&self, purpose: ChannelPurpose) -> Result<Arc<dyn TransportChannel>>;

    /// List all open channels
    fn channels(&self) -> Vec<ChannelPurpose>;

    /// Check if transport supports multiple channels
    fn supports_multi_channel(&self) -> bool { false }

    // No default async send/recv here; callers should use the control channel explicitly.
}
```

#### 1.3 WebRTC Multi-Channel Implementation

**File**: `apps/beach/src/transport/webrtc/mod.rs` (modify)

```rust
pub struct WebRTCTransport {
    peer_connection: Arc<RTCPeerConnection>,
    channels: Arc<RwLock<HashMap<ChannelPurpose, Arc<WebRTCChannel>>>>,
    mode: TransportMode,
    next_message_id: Arc<AsyncMutex<u32>>,
}

impl WebRTCTransport {
    async fn create_channel_internal(
        &self,
        purpose: ChannelPurpose,
        reliability: ChannelReliability,
    ) -> Result<Arc<WebRTCChannel>> {
        let label = match purpose {
            ChannelPurpose::Control => "beach/ctrl/1",
            ChannelPurpose::Output => "beach/term/1",
            ChannelPurpose::Custom(n) => &format!("beach/custom/{}", n),
        };
        
        let init = match reliability {
            ChannelReliability::Reliable => RTCDataChannelInit {
                ordered: Some(true),
                ..Default::default()
            },
            ChannelReliability::Unreliable { max_retransmits, .. } => RTCDataChannelInit {
                ordered: Some(false),
                max_retransmits, // set Some(0) for best‑effort
                ..Default::default()
            },
        };
        
        let rtc_channel = self.peer_connection.create_data_channel(label, Some(init)).await?;
        // ... setup channel handlers ...
    }
}
```

### Phase 2: Message Routing

#### 2.1 Channel-Aware Message Types

**File**: `apps/beach/src/protocol/channel_messages.rs` (new)

```rust
/// Messages that MUST go through reliable control channel
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlMessage {
    // From client
    TerminalInput { data: Vec<u8> },
    Acknowledge { version: u64 },
    ResyncRequest { reason: String },
    Viewport { cols: u16, rows: u16 },
    Heartbeat { timestamp: i64 },
    
    // From server
    HeartbeatAck { timestamp: i64 },
    Hash { version: u64, hash: [u8; 32] },
    ResyncReady { version: u64 },
}

/// Messages that CAN go through unreliable output channel
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OutputMessage {
    Delta {
        base_version: u64,
        next_version: u64,
        delta: GridDelta,
    },
    Snapshot {
        version: u64,
        grid: Grid,
        compressed: bool,
    },
}
```

#### 2.2 Message Router

**File**: `apps/beach/src/session/channel_router.rs` (new)

```rust
pub struct ChannelRouter<T: Transport> {
    transport: T,
    control_tx: mpsc::Sender<ControlMessage>,
    output_tx: mpsc::Sender<OutputMessage>,
}

impl<T: Transport> ChannelRouter<T> {
    pub async fn route_outgoing(&mut self, msg: ServerMessage) -> Result<()> {
        match msg {
            // Control channel messages
            ServerMessage::Pong { .. } |
            ServerMessage::Error { .. } => {
                let control = self.transport.channel(ChannelPurpose::Control).await?;
                control.send(&serialize(&msg)?).await?;
            }
            
            // Output channel messages (if available)
            ServerMessage::Delta { .. } |
            ServerMessage::Snapshot { .. } => {
                if self.transport.supports_multi_channel() {
                    let output = self.transport.channel(ChannelPurpose::Output).await?;
                    output.send(&serialize(&msg)?).await?;
                } else {
                    // Fallback to control channel
                    let control = self.transport.channel(ChannelPurpose::Control).await?;
                    control.send(&serialize(&msg)?).await?;
                }
            }
            
            _ => { /* other messages */ }
        }
    }
}
```

### Phase 3: Security Implementation

#### 3.1 Remove Passphrase from Signaling

**File**: `apps/beach/src/session/mod.rs` (modify)

```rust
impl<T: Transport> ServerSession<T> {
    pub async fn connect_signaling(&mut self, session_server: &str, session_id: &str) -> Result<()> {
        // REMOVE: self.session.passphrase.clone()
        let connection = create_websocket_signaling(
            session_server,
            session_id,
            self.session.id.clone(),
            None, // Don't send passphrase!
            PeerRole::Server,
        ).await?;
        
        // Store passphrase hash locally for verification
        if let Some(passphrase) = &self.session.passphrase {
            self.passphrase_verifier = Some(PassphraseVerifier::new(passphrase));
        }
    }
}
```

#### 3.2 Sealed Signaling (detailed)

**File**: `apps/beach/src/transport/webrtc/sealed.rs` (new)

```rust
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use argon2::{Argon2, Params};

pub struct SealedSignaling {
    key: Key,
}

impl SealedSignaling {
    pub fn from_passphrase(passphrase: &str, session_id: &str, salt: [u8;16]) -> Result<Self> {
        // Derive key using Argon2id with per‑session random salt
        let params = Params::new(64 * 1024, 3, 2, None)?; // 64MB, 3 iters, 2 lanes (tunable)
        let mut key = [0u8; 32];
        Argon2::default().hash_password_into(passphrase.as_bytes(), &salt, &mut key)?;
        Ok(Self { key: Key::from_slice(&key).clone() })
    }

    pub fn seal_sdp(&self, sdp: &str, ad: &AssociatedData) -> Result<Envelope> {
        // Plaintext includes SDP, fingerprint and a timestamp for freshness
        let plaintext = json!({
            "sdp": sdp,
            "fingerprint": extract_fingerprint(sdp)?,
            "ts": Utc::now().to_rfc3339(),
        }).to_string();

        let nonce = generate_nonce_96();
        let aead = ChaCha20Poly1305::new(&self.key);
        let aad_bytes = serde_json::to_vec(ad)?; // includes session_id, roles, from/to_peer
        let ciphertext = aead.encrypt(&Nonce::from_slice(&nonce), chacha20poly1305::aead::Payload { msg: plaintext.as_bytes(), aad: &aad_bytes })?;

        Ok(Envelope { v: "ssv1", typ: "sdp", salt: base64::encode(ad.salt), nonce: base64::encode(nonce), ad: ad.clone(), sealed: base64::encode(ciphertext) })
    }
}
```

#### 3.3 Application Handshake (Noise now, PAKE later)

**File**: `apps/beach/src/transport/webrtc/handshake.rs` (new)

```rust
// Run over the reliable control channel. Bind to DTLS via exporter.
// Initial implementation: Noise XXpsk2 (shared secret from passphrase), using `snow`.
// Prologue includes: 32‑byte DTLS exporter (label "beach/bind/1"), hash of sealed signaling envelopes, channel label.
// Clerk mode: send Clerk token inside the encrypted payload; server verifies with JWKS and enforces policy.

// Pseudocode sketch (server side):
// let exporter = dtls_exporter("beach/bind/1", None, 32);
// let prologue = concat(exporter, hash(sealed_transcript), label_bytes);
// let noise = snow::Builder::new("Noise_XXpsk2_25519_ChaChaPoly_BLAKE2s").psk(2, psk).prologue(&prologue).build_responder()?;
// -> exchange handshake messages over control channel; on success, derive K and split into k_tx/k_rx/k_control.
```

### Phase 4: Frame Versioning & Resync

#### 4.1 Version Tracking

**File**: `apps/beach/src/server/terminal_state/versioning.rs` (new)

```rust
pub struct VersionedFrameManager {
    current_version: AtomicU64,
    delta_window: Arc<RwLock<VecDeque<VersionedDelta>>>,
    window_duration: Duration,
    last_snapshot: Arc<RwLock<Option<VersionedSnapshot>>>,
}

pub struct VersionedDelta {
    pub base_version: u64,
    pub next_version: u64,
    pub delta: GridDelta,
    pub timestamp: Instant,
}

impl VersionedFrameManager {
    pub fn next_delta(&self, delta: GridDelta, client_ack: u64) -> OutputMessage {
        let base = client_ack;
        let next = self.current_version.fetch_add(1, Ordering::SeqCst) + 1;
        
        // Store in window
        let versioned = VersionedDelta {
            base_version: base,
            next_version: next,
            delta,
            timestamp: Instant::now(),
        };
        
        self.delta_window.write().unwrap().push_back(versioned.clone());
        self.trim_window();
        
        OutputMessage::Delta {
            base_version: base,
            next_version: next,
            delta: versioned.delta,
        }
    }
    
    pub fn resync(&self, requested_version: u64) -> OutputMessage {
        // Try to build delta chain from requested version
        if let Some(chain) = self.build_delta_chain(requested_version) {
            OutputMessage::DeltaChain {
                base_version: requested_version,
                deltas: chain,
            }
        } else {
            // Fall back to snapshot
            OutputMessage::Snapshot {
                version: self.current_version.load(Ordering::SeqCst),
                grid: self.last_snapshot.read().unwrap().grid.clone(),
                compressed: true,
            }
        }
    }
}
```

#### 4.2 Client Acknowledgment

**File**: `apps/beach/src/client/frame_tracker.rs` (new)

```rust
pub struct ClientFrameTracker {
    last_applied_version: u64,
    pending_deltas: BTreeMap<u64, GridDelta>,
    ack_interval: Duration,
    last_ack: Instant,
}

impl ClientFrameTracker {
    pub async fn handle_frame(&mut self, msg: OutputMessage, control: &mut dyn TransportChannel) -> Result<()> {
        match msg {
            OutputMessage::Delta { base_version, next_version, delta } => {
                if base_version == self.last_applied_version {
                    // Can apply immediately
                    self.apply_delta(delta)?;
                    self.last_applied_version = next_version;
                    
                    // Send ack if interval elapsed
                    if self.last_ack.elapsed() > self.ack_interval {
                        control.send(&serialize(&ControlMessage::Acknowledge {
                            version: self.last_applied_version,
                        })?).await?;
                        self.last_ack = Instant::now();
                    }
                } else if base_version > self.last_applied_version {
                    // Gap detected, request resync
                    control.send(&serialize(&ControlMessage::ResyncRequest {
                        reason: format!("Gap detected: {} -> {}", self.last_applied_version, base_version),
                    })?).await?;
                } else {
                    // Out of order, store for later
                    self.pending_deltas.insert(base_version, delta);
                }
            }
            OutputMessage::Snapshot { version, grid, .. } => {
                // Full resync
                self.apply_snapshot(grid)?;
                self.last_applied_version = version;
                self.pending_deltas.clear();
            }
        }
        Ok(())
    }
}
```

### 2.3 Client Deliverables for Phase 2a (Minimal Client)

Goal: deliver a vertical slice proving end‑to‑end behavior over the reliable control channel. Output may flow over control as a fallback until the unreliable channel is enabled in Phase 2b.

Must‑have behaviors:
- Passphrase interstitial (public mode) if `--passphrase`/env/profile not set.
- Open reliable control channel and render initial snapshot.
- Predictive echo: underline local typed characters; remove underline on ack (stub ok if acks not yet wired).
- Enforce server width; no soft wrapping. Provide horizontal panning indicators when local terminal narrower.
- Vertical scrolling with overscan subscription (request `visible_rows * 2` and update `from_line` on scroll).
- Resilience: keep last screen on disconnect; auto reconnect; request fresh snapshot on resume.

Tests (Rust): `apps/beach/src/tests/client/`
- `join_flow.rs`: join → control open → initial render.
- `predictive_input.rs`: underline on type; underline removed on ack; correction on conflicting output.
- `scroll_prefetch.rs`: overscan subscribe and instant scrollback.
- `reconnect_resync.rs`: disconnect → resume with snapshot.
- `order_guarantees.rs`: two clients interleave inputs → server serialization reflected in acks.

### 2.4 Portable Control/Output Message Shapes

Control (reliable):
- `Input { client_id: String, client_seq: u64, bytes: Vec<u8> }`
- `InputAck { client_seq: u64, apply_seq: u64, version: u64 }`
- `Ack { version: u64 }`
- `ResyncRequest { reason: String }`
- `Viewport { cols: u16, rows: u16 }`
- `Subscribe { from_line: u64, height: u16 }`
- `Heartbeat { t: i64 }`, `HeartbeatAck { t: i64 }`

Output (unreliable preferred):
- `Delta { base_version: u64, next_version: u64, delta: GridDelta }`
- `Snapshot { version: u64, grid: Grid, compressed: bool }`
- `Hash { version: u64, h: [u8; 32] }`

Note: these shapes must be identical across Rust and a future TS client; consider JSON schema or protobuf with deterministic encoding.

## Configuration

### Environment Variables

```bash
# Channel configuration
BEACH_DUAL_CHANNEL=true                    # Enable dual-channel mode
BEACH_OUTPUT_UNRELIABLE=true              # Use unreliable output channel
BEACH_MAX_RETRANSMITS=0                   # For unreliable channel
BEACH_DELTA_WINDOW_MS=3000                # Keep 3 seconds of deltas

# Security
BEACH_SEALED_SIGNALING=true               # Enable sealed signaling (Public & Private)
BEACH_REQUIRE_HANDSHAKE=true              # Require app-level handshake on control channel
BEACH_ARGON2_MEM_KIB=65536               # 64MB for key derivation
BEACH_ARGON2_ITERATIONS=3                 # Time cost

# Frame management
BEACH_ACK_INTERVAL_MS=75                  # Client ack frequency
BEACH_SNAPSHOT_INTERVAL_MS=5000           # Force snapshot every 5s
BEACH_MAX_DELTA_CHAIN=20                  # Max deltas in resync

# Mode & profiles
BEACH_MODE=public                         # public|private (default auto from credentials)
BEACH_PROFILE=default                     # Selected profile
BEACH_SESSION_SERVER=localhost:8080       # Session server host (public/private inferred)
```

## Migration Strategy

### Stage 1: Preparation (Week 1)
- [ ] Implement channel abstraction without breaking changes
- [ ] Add multi-channel support to WebRTC with fallback
- [ ] Remove passphrase from signaling (CRITICAL)

### Stage 2: Dual Channel (Week 2)
- [ ] Deploy channel router
- [ ] Enable output channel in test environments
- [ ] Monitor performance metrics

### Stage 3: Security (Week 3)
- [ ] Add sealed signaling with feature flag
- [ ] Implement basic handshake
- [ ] Gate PTY access behind authentication

### Stage 4: Optimization (Week 4)
- [ ] Add frame versioning
- [ ] Implement resync protocol
- [ ] Tune parameters based on metrics

## Testing Strategy

### Unit Tests
- Channel creation and reliability settings
- Message routing logic
- Sealed signaling encryption/decryption
- Frame versioning and resync

### Integration Tests
- Dual-channel connection establishment
- Fallback to single channel
- Handshake success/failure
- Large message chunking across channels

### Performance Tests
- Latency comparison: single vs dual channel
- Throughput under packet loss
- Resync recovery time
- CPU/memory overhead

## Risk Assessment

### Technical Risks
1. **WebRTC Library Limitations**: May need to fork/patch webrtc-rs
2. **Browser Compatibility**: Some browsers may not support all channel configs
3. **DTLS Exporter Access**: May not be exposed by library

### Mitigation
- Maintain single-channel fallback
- Feature flags for gradual rollout
- Comprehensive testing before production

## Success Metrics

- **Security**: Zero passphrase leaks to signaling server
- **Latency**: 30% reduction in p99 input-to-output latency
- **Resilience**: <100ms recovery from 5% packet loss
- **Compatibility**: 100% backward compatibility with single-channel clients

## Open Questions

1. Should we use SPAKE2+ or Noise for the handshake initially?
2. What compression algorithm for snapshots? (zstd vs lz4)
3. How to handle channel failure? (retry vs fallback)
4. Should frame versions be globally unique or per-connection?

## Next Steps

1. Review and approve this plan
2. Create detailed API specifications
3. Set up test infrastructure
4. Begin Phase 1 implementation

---

*This document will be updated as implementation progresses and decisions are made.*

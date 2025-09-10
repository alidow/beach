# Subscription Manager Design Proposal

## Overview

This document proposes a significant refactoring of the subscription system in Beach to create a cleaner, more flexible architecture. The core idea is to make SubscriptionManager the single, unified interface for all subscription-based I/O between server and client, with WebRTC as the default data transport and explicit, opt‑in debug transports when needed.

## Summary of Refinements

- Prefer `Arc<dyn Transport>` over a transport enum inside subscriptions to keep the layer transport‑agnostic and aligned with current abstractions.
- Move channel‑purpose routing (Control vs Output) into SubscriptionManager so callers never decide which data channel to use.
- **Dual WebRTC channels handled transparently**: SubscriptionManager automatically routes deltas to unreliable channel, critical messages to reliable channel.
- Decouple server state via a `TerminalDataSource` trait injected into the manager (instead of directly depending on `TerminalStateTracker`).
- Keep WebSocket for signaling only in production; allow WebSocket data paths strictly behind an explicit debug flag/mode.
- Make the input path single‑sourced through the manager's handler (avoid duplicate PTY paths).
- Replace periodic snapshots with event‑driven deltas sourced from the terminal backend/history.

## Current Problems

1. **Architectural Asymmetry**: 
   - Server uses SessionBridgeHandler with SubscriptionManager
   - Client has no SubscriptionManager at all
   - Different patterns for handling subscription messages

2. **Transport Confusion**:
   - SessionBridgeHandler incorrectly routes subscriptions through WebSocket
   - MockTransport used as a placeholder in SubscriptionManager
   - Violates design principle: WebSocket for signaling only, WebRTC for data

3. **Tight Coupling**:
   - SubscriptionManager directly accesses TerminalStateTracker
   - Business logic mixed with transport/routing logic
   - Hard to test and reuse

4. **Inflexibility**:
   - No way to use different transports for debugging
   - Can't easily connect debug tools or monitoring clients

## Proposed Architecture

### Core Principles

1. **SubscriptionManager as Unified Interface**: Both server and client use SubscriptionManager for all subscription I/O
2. **Session Ownership**: SubscriptionManager belongs to Session, not buried in handlers
3. **Transport Flexibility**: Each subscription can specify its transport type
4. **Clean Separation**: Transport/routing logic separate from business logic

### Structural Changes

```rust
// Session owns SubscriptionManager
pub struct Session<T: Transport> {
    id: String,
    url: SessionUrl,
    transport: Arc<Mutex<T>>,  // Primary transport (WebRTC)
    subscription_manager: Arc<SubscriptionManager>,
    signaling_transport: Option<Arc<SignalingTransport>>,  // WebSocket for signaling
    passphrase: Option<String>,
}

// SubscriptionManager is transport-agnostic
pub struct SubscriptionManager {
    subscriptions: Arc<RwLock<HashMap<SubscriptionId, Subscription>>>,
    clients: Arc<RwLock<HashMap<ClientId, Vec<SubscriptionId>>>>,
    // No transport field - uses per-subscription transports
    // No terminal_tracker - server provides data on demand
}

// Each subscription specifies its transport
pub struct Subscription {
    pub id: SubscriptionId,
    pub client_id: ClientId,
    pub dimensions: Dimensions,
    pub mode: ViewMode,
    pub position: Option<ViewPosition>,
    pub transport: SubscriptionTransport,  // NEW: Per-subscription transport
    pub is_controlling: bool,
    pub last_sequence_acked: u64,
    pub current_sequence: u64,
    // Remove grid_view - server calculates on demand
    // Remove connection - use transport instead
}

pub enum SubscriptionTransport {
    WebRTC(Arc<WebRTCTransport>),
    WebSocket(Arc<WebSocketTransport>),  // For debugging/special cases
    Mock(Arc<MockTransport>),  // For testing
}
```

### Refined Structural Changes (recommended)

```rust
// Session owns SubscriptionManager and the primary data transport (WebRTC)
pub struct Session<T: Transport> {
    id: String,
    url: SessionUrl,
    transport: Arc<Mutex<T>>,                 // WebRTC in production
    subscription_manager: Arc<SubscriptionManager>,
    signaling_transport: Option<Arc<SignalingTransport>>, // WebSocket (signaling only)
    passphrase: Option<String>,
}

// SubscriptionManager uses dyn Transport and routes channels internally
pub struct SubscriptionManager {
    subscriptions: Arc<RwLock<HashMap<SubscriptionId, Subscription>>>,
    clients: Arc<RwLock<HashMap<ClientId, Vec<SubscriptionId>>>>,
    terminal_source: Option<Arc<dyn TerminalDataSource + Send + Sync>>, // server-only injection
}

pub struct Subscription {
    pub id: SubscriptionId,
    pub client_id: ClientId,
    pub dimensions: Dimensions,
    pub mode: ViewMode,
    pub position: Option<ViewPosition>,
    pub transport: Arc<dyn Transport>,        // per-subscription (defaults to session transport)
    pub is_controlling: bool,
    pub last_sequence_acked: u64,
    pub current_sequence: u64,
}

#[async_trait]
pub trait TerminalDataSource {
    async fn snapshot(&self, dims: Dimensions) -> anyhow::Result<Grid>;
    async fn next_delta(&self) -> anyhow::Result<GridDelta>; // await next change (coalesced)
}
```

### API Design

```rust
impl SubscriptionManager {
    // Core lifecycle
    pub async fn add_subscription(
        &self,
        id: SubscriptionId,
        client_id: ClientId,
        config: SubscriptionConfig,
        transport: Arc<dyn Transport>,
        handler: Arc<dyn SubscriptionHandler>,
    ) -> Result<()>;

    pub async fn remove_subscription(&self, id: &SubscriptionId) -> Result<()>;

    // Data routing: chooses channel purpose internally
    pub async fn send(&self, client_id: &ClientId, message: ServerMessage) -> Result<()>; // Output channel for frames
    pub async fn broadcast(&self, message: ServerMessage) -> Result<()>;

    // Incoming client protocol
    pub async fn handle_incoming(&self, client_id: &ClientId, message: ClientMessage) -> Result<()>;
}

// Handler trait for business logic
#[async_trait]
pub trait SubscriptionHandler: Send + Sync {
    async fn on_subscribe(&self, subscription: &Subscription) -> Result<ServerMessage>; // initial snapshot
    async fn on_input(&self, subscription: &Subscription, data: Vec<u8>) -> Result<()>; // PTY input on server
    async fn on_resize(&self, subscription: &Subscription, dimensions: Dimensions) -> Result<()>;
    async fn on_unsubscribe(&self, subscription: &Subscription) -> Result<()>;
}
```

### Dual Channel Management (WebRTC)

When using WebRTC transport, SubscriptionManager transparently handles dual data channels:

#### Channel Assignment
The SubscriptionManager automatically routes messages to the appropriate channel based on message type:

**Unreliable/Output Channel** (loss-tolerant, high-frequency):
- `ServerMessage::Delta` - Individual terminal changes
- `ServerMessage::DeltaBatch` - Batched terminal changes  
- Other high-frequency frame updates

**Reliable/Control Channel** (guaranteed delivery):
- `ServerMessage::Snapshot` - Full grid state (critical for sync)
- `ServerMessage::SubscriptionAck` - Subscription confirmations
- `ServerMessage::Error` - Error notifications
- `ClientMessage::Subscribe` - Subscription requests
- `ClientMessage::Input` - User keyboard input (must not be lost)
- `ClientMessage::Resize` - Terminal dimension changes
- Control messages (Ping/Pong, Resync, Viewport adjustments)

#### Implementation Details

```rust
impl SubscriptionManager {
    async fn send(&self, client_id: &ClientId, message: ServerMessage) -> Result<()> {
        let subscription = self.get_subscription(client_id)?;
        
        match &subscription.transport {
            // WebRTC transport knows about dual channels
            transport if transport.supports_dual_channels() => {
                match message {
                    ServerMessage::Delta { .. } | 
                    ServerMessage::DeltaBatch { .. } => {
                        // High-frequency updates go unreliable
                        transport.send_unreliable(message).await
                    },
                    _ => {
                        // Everything else needs reliability
                        transport.send_reliable(message).await
                    }
                }
            },
            // Fallback transports (WebSocket, Mock) are always reliable
            transport => transport.send(message).await
        }
    }
}
```

#### Abstraction Benefits

Terminal servers and clients **never need to know about channel selection**. They simply:
- Call `subscription_manager.send()` with their message
- Trust SubscriptionManager to route appropriately
- Focus on business logic (generating snapshots, processing input, etc.)

This abstraction ensures:
- Consistent channel usage across the codebase
- Easy to change routing rules in one place
- Transport details hidden from business logic
- Graceful degradation for single-channel transports

## Subscription‑Centric API (Revised)

Make the hub subscription‑centric so the server never handles client identities during I/O. The hub owns sub↔transport mapping, pulls deltas from the terminal source, and fans out frames via the correct data channel. Clients get a demuxed stream per subscription.

### Server‑Side API

```rust
impl SubscriptionManager {
    /// Inject the source of terminal state (server only)
    pub fn attach_source(&self, source: Arc<dyn TerminalDataSource + Send + Sync>);

    /// Attach a PTY writer for input (server only)
    pub fn set_pty_writer(&self, writer: Arc<dyn PtyWriter + Send + Sync>);

    /// Create a subscription bound to a client's transport
    pub async fn subscribe(
        &self,
        client_transport: Arc<dyn Transport>,
        config: SubscriptionConfig,
    ) -> anyhow::Result<SubscriptionId>;

    /// Update an existing subscription's view parameters
    pub async fn update(&self, id: &SubscriptionId, patch: SubscriptionUpdate) -> anyhow::Result<()>;

    /// Remove a subscription
    pub async fn unsubscribe(&self, id: &SubscriptionId) -> anyhow::Result<()>;

    /// Start event‑driven streaming (initial snapshots + deltas)
    pub fn start_streaming(&self) -> tokio::task::JoinHandle<()>;

    /// Force a snapshot (resync) for one subscription
    pub async fn force_snapshot(&self, id: &SubscriptionId) -> anyhow::Result<()>;

    /// Optional manual delta push (advanced/testing)
    pub async fn push_delta(&self, delta: GridDelta) -> anyhow::Result<()>;

    /// Handle incoming client protocol for a subscription (input, resize, etc.)
    pub async fn handle_incoming(&self, id: &SubscriptionId, msg: ClientMessage) -> anyhow::Result<()>;
}

pub struct SubscriptionUpdate {
    pub dimensions: Option<Dimensions>,
    pub mode: Option<ViewMode>,
    pub position: Option<ViewPosition>,
    pub is_controlling: Option<bool>,
}

#[async_trait]
pub trait PtyWriter {
    async fn write(&self, bytes: &[u8]) -> anyhow::Result<()>;
}
```

Behavior:
- On `start_streaming`, the hub sends an initial `Snapshot` for each live subscription using `TerminalDataSource::snapshot`, then awaits `next_delta()` and emits `Delta`/`DeltaBatch` to all live subs.
- Channel routing (Control vs Output) happens inside the hub per message type.
- Input (TerminalInput) is routed to `PtyWriter` only for `is_controlling` subscriptions.

### Client‑Side API

```rust
impl SubscriptionManager {
    /// Open a remote subscription; returns id and a demuxed stream of frames
    pub async fn open_remote_subscription(
        &self,
        config: SubscriptionConfig,
    ) -> anyhow::Result<(SubscriptionId, tokio::sync::mpsc::Receiver<ServerMessage>)>;

    /// Close a remote subscription
    pub async fn close_remote_subscription(&self, id: &SubscriptionId) -> anyhow::Result<()>;

    /// Send input for a given subscription (control channel)
    pub async fn send_input(&self, id: &SubscriptionId, bytes: Vec<u8>) -> anyhow::Result<()>;
}
```

### Usage Patterns

#### Server Side

```rust
// In TerminalServer
impl TerminalServer {
    pub async fn new(config: Config, transport: T) -> Result<Self> {
        let session = ServerSession::new(transport);
        
        // Server registers handler for subscription events
        let handler = ServerSubscriptionHandler {
            terminal_tracker: Arc::new(Mutex::new(terminal_tracker)),
        };
        
        session.subscription_manager.set_handler(Box::new(handler));
        
        // ...
    }
}

impl SubscriptionHandler for ServerSubscriptionHandler {
    async fn on_subscribe(&self, subscription: &Subscription) -> Result<ServerMessage> {
        // Server calculates snapshot on demand
        let tracker = self.terminal_tracker.lock().unwrap();
        let grid_view = tracker.create_view(subscription.dimensions);
        let snapshot = grid_view.to_snapshot();
        
        Ok(ServerMessage::Snapshot { 
            subscription_id: subscription.id.clone(),
            grid: snapshot,
            // ...
        })
    }
    
    async fn on_input(&self, subscription: &Subscription, data: Vec<u8>) -> Result<()> {
        // Forward input to PTY if subscription is controlling
        if subscription.is_controlling {
            self.pty.write(&data)?;
        }
        Ok(())
    }
}
```

#### Client Side

```rust
// In TerminalClient
impl TerminalClient {
    pub async fn new(config: Config, transport: T) -> Result<Self> {
        let session = ClientSession::new(transport);
        
        // Client uses SubscriptionManager API
        let subscription_id = session.subscription_manager.subscribe(
            SubscriptionConfig {
                dimensions: terminal_dimensions(),
                mode: ViewMode::Realtime,
                position: None,
            },
            // Default to session's WebRTC transport (Strict mode)
            session.transport.clone() as Arc<dyn Transport>,
            Box::new(ClientSubscriptionHandler { /* ... */ }),
        ).await?;
        
        // ...
    }
}
```

### Transport Selection

```rust
pub struct SubscriptionConfig {
    pub dimensions: Dimensions,
    pub mode: ViewMode,
    pub position: Option<ViewPosition>,
    pub transport_mode: TransportMode,
}

pub enum TransportMode {
    Strict,         // WebRTC only (default)
    Flexible,       // Prefer WebRTC, allow WS when explicitly enabled
    ForceWebSocket, // Debug only
}

impl SubscriptionManager {
    pub async fn add_subscription(
        &self,
        config: SubscriptionConfig,
        available_transports: AvailableTransports,
    ) -> Result<Arc<dyn Transport>> {
        match config.transport_mode {
            TransportMode::Strict => {
                available_transports.webrtc
                    .map(|t| t as Arc<dyn Transport>)
                    .ok_or_else(|| anyhow!("WebRTC required in strict mode"))
            }
            TransportMode::Flexible => {
                available_transports.webrtc
                    .map(|t| t as Arc<dyn Transport>)
                    .or(available_transports.websocket.map(|t| t as Arc<dyn Transport>))
                    .ok_or_else(|| anyhow!("No transport available"))
            }
            TransportMode::ForceWebSocket => {
                available_transports.websocket
                    .map(|t| t as Arc<dyn Transport>)
                    .ok_or_else(|| anyhow!("WebSocket not available"))
            }
        }
    }
}
```

## Benefits

1. **Unified Architecture**: Same patterns for client and server
2. **Clean Separation**: Business logic separated via handler; transport abstracted via `dyn Transport`
3. **Correct Channels**: Dual WebRTC channels (reliable/unreliable) managed transparently, preventing misroutes
4. **Flexibility**: Explicit, opt‑in per‑subscription transport for debug tools
5. **Testability**: Use MockTransport and a `TerminalDataSource` mock for unit tests
6. **Simplicity**: Remove SessionBridgeHandler and complex bridging logic
7. **Correctness/Security**: WebRTC for data by default; WS for signaling (or debug only)

## Migration Strategy

### Phase 0: Event-driven frames
- Hook terminal backend/history to publish deltas; remove periodic snapshot polling

### Phase 1: Refactor SubscriptionManager
- Introduce `TerminalDataSource` trait; remove direct tracker dependency
- Add channel‑purpose routing inside manager
- Convert to handler‑based pattern

### Phase 2: Move to Session
- Move SubscriptionManager ownership to Session (client and server)
- Update Session routers to route AppMessage directly to the manager

### Phase 3: Remove SessionBridgeHandler
- Route subscription messages directly via manager; remove CompositeHandler
- Keep a minimal compatibility shim during rollout if needed

### Phase 4: Update Client
- Add SubscriptionManager to client
- Use unified API for subscriptions

## Debug Mode Use Cases

With per-subscription transport selection, we can support:

1. **Debug Client**: Connect via WebSocket to inspect terminal state without full WebRTC setup
2. **Monitoring Tools**: Lightweight connections for observability
3. **Testing**: Use MockTransport for unit tests
4. **Development**: Easier debugging with WebSocket fallback

Example:
```bash
# Normal client (WebRTC)
beach --join session-id

# Debug client (WebSocket)
beach --join session-id --debug-transport websocket

# Monitoring tool
beach-monitor --session session-id --transport websocket --read-only
```

## Security Considerations

1. **Default Strict Mode**: Production uses WebRTC only for data
2. **Explicit Opt-in**: WebSocket data paths require an explicit debug mode/flag; never auto‑fallback in production
3. **Authentication**: Same auth/handshake policy applied regardless of transport; ensure WS paths do not leak secrets
4. **Isolation**: Debug subscriptions are read‑only by default and scoped to narrow privileges

## Testing Strategy

1. **Unit Tests**: MockTransport for SubscriptionManager logic
2. **Integration Tests**: Test both WebRTC and WebSocket paths
3. **Migration Tests**: Ensure backward compatibility during rollout
4. **Performance Tests**: Verify no regression in latency/throughput

## Open Questions

## Naming

While “SubscriptionManager” is serviceable, a name that reflects central routing and fan‑out can improve readability:

- SubscriptionHub: emphasizes central ownership of subscriptions and routing
- StreamRouter: focuses on dual‑channel selection and delivery
- TerminalStreamHub: domain‑specific clarity (terminal frames + control)

Recommendation: rename to SubscriptionHub. It remains transport‑agnostic via `dyn Transport`, encapsulates dual‑channel policy, and matches its role as the single entry point for subscription I/O.

1. Should we support mixed transports in a single session (some subscriptions on WebRTC, others on WebSocket)?
   - **Recommendation**: Yes, for maximum flexibility in debugging

2. How do we handle transport fallback if WebRTC fails?
   - **Recommendation**: Explicit opt-in only, never automatic fallback in production

3. Should transport selection be per-session or per-subscription?
   - **Recommendation**: Per-subscription for maximum flexibility

## Conclusion

This refactoring will significantly simplify the subscription architecture while adding valuable debugging capabilities. By making SubscriptionManager the unified interface and allowing per-subscription transport selection, we achieve both architectural cleanliness and practical flexibility.

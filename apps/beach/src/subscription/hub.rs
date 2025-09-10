use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use anyhow::{Result, anyhow};
use async_trait::async_trait;

use crate::protocol::{
    ClientMessage, ServerMessage, ViewMode, ViewPosition, Dimensions,
    ErrorCode, SubscriptionStatus, NotificationType
};
use crate::transport::{Transport, ChannelPurpose};
use super::{SubscriptionId, ClientId};
use super::data_source::{TerminalDataSource, PtyWriter};

/// Configuration for a subscription
#[derive(Clone, Debug)]
pub struct SubscriptionConfig {
    pub dimensions: Dimensions,
    pub mode: ViewMode,
    pub position: Option<ViewPosition>,
    pub is_controlling: bool,
}

/// Updates to apply to an existing subscription
#[derive(Clone, Debug, Default)]
pub struct SubscriptionUpdate {
    pub dimensions: Option<Dimensions>,
    pub mode: Option<ViewMode>,
    pub position: Option<ViewPosition>,
    pub is_controlling: Option<bool>,
}

/// A single subscription with its associated transport
struct Subscription {
    id: SubscriptionId,
    client_id: ClientId,
    dimensions: Dimensions,
    mode: ViewMode,
    position: Option<ViewPosition>,
    transport: Arc<dyn Transport>,
    is_controlling: bool,
    last_sequence_acked: u64,
    current_sequence: u64,
}

/// Handler trait for subscription business logic
#[async_trait]
pub trait SubscriptionHandler: Send + Sync {
    /// Called when a new subscription is created
    async fn on_subscribe(&self, id: &SubscriptionId, config: &SubscriptionConfig) -> Result<()>;
    
    /// Called when input is received from a controlling subscription
    async fn on_input(&self, id: &SubscriptionId, data: Vec<u8>) -> Result<()>;
    
    /// Called when a subscription requests a resize
    async fn on_resize(&self, id: &SubscriptionId, dimensions: Dimensions) -> Result<()>;
    
    /// Called when a subscription is removed
    async fn on_unsubscribe(&self, id: &SubscriptionId) -> Result<()>;
}

/// Central hub for managing all subscriptions
/// This is transport-agnostic and handles channel routing transparently
pub struct SubscriptionHub {
    subscriptions: Arc<RwLock<HashMap<SubscriptionId, Subscription>>>,
    clients: Arc<RwLock<HashMap<ClientId, Vec<SubscriptionId>>>>,
    
    // Server-only: source of terminal data
    terminal_source: Arc<RwLock<Option<Arc<dyn TerminalDataSource>>>>,
    
    // Server-only: PTY writer for input
    pty_writer: Arc<RwLock<Option<Arc<dyn PtyWriter>>>>,
    
    // Optional handler for business logic
    handler: Arc<RwLock<Option<Arc<dyn SubscriptionHandler>>>>,
    
    // Channel for delta streaming task
    delta_tx: Arc<RwLock<Option<mpsc::Sender<()>>>>,
    
    // Debug log path for verbose logging
    debug_log_path: Arc<RwLock<Option<String>>>,
}

impl SubscriptionHub {
    /// Create a new subscription hub
    pub fn new() -> Self {
        Self {
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            clients: Arc::new(RwLock::new(HashMap::new())),
            terminal_source: Arc::new(RwLock::new(None)),
            pty_writer: Arc::new(RwLock::new(None)),
            handler: Arc::new(RwLock::new(None)),
            delta_tx: Arc::new(RwLock::new(None)),
            debug_log_path: Arc::new(RwLock::new(None)),
        }
    }

    /// Check if any subscription exists for a given client
    pub async fn has_any_for_client(&self, client_id: &ClientId) -> bool {
        let clients = self.clients.read().await;
        clients.get(client_id).map(|v| !v.is_empty()).unwrap_or(false)
    }
    
    /// Set debug log path for verbose logging
    pub async fn set_debug_log_path(&self, path: String) {
        *self.debug_log_path.write().await = Some(path);
    }
    
    /// Attach a terminal data source (server-only)
    pub async fn attach_source(&self, source: Arc<dyn TerminalDataSource>) {
        *self.terminal_source.write().await = Some(source);
    }
    
    /// Attach a PTY writer (server-only)
    pub async fn set_pty_writer(&self, writer: Arc<dyn PtyWriter>) {
        *self.pty_writer.write().await = Some(writer);
    }
    
    /// Set the subscription handler
    pub async fn set_handler(&self, handler: Arc<dyn SubscriptionHandler>) {
        *self.handler.write().await = Some(handler);
    }
    
    /// Create a new subscription with server-assigned ID
    pub async fn subscribe(
        &self,
        client_id: ClientId,
        client_transport: Arc<dyn Transport>,
        config: SubscriptionConfig,
    ) -> Result<SubscriptionId> {
        let subscription_id = uuid::Uuid::new_v4().to_string();
        
        let subscription = Subscription {
            id: subscription_id.clone(),
            client_id: client_id.clone(),
            dimensions: config.dimensions.clone(),
            mode: config.mode.clone(),
            position: config.position.clone(),
            transport: client_transport.clone(),
            is_controlling: config.is_controlling,
            last_sequence_acked: 0,
            current_sequence: 0,
        };
        
        // Send acknowledgment
        self.send_to_subscription(&subscription, ServerMessage::SubscriptionAck {
            subscription_id: subscription_id.clone(),
            status: SubscriptionStatus::Active,
            shared_with: None,
        }).await?;
        
        // Send initial snapshot if we have a data source
        if let Some(source) = self.terminal_source.read().await.as_ref() {
            if let Some(ref debug_log_path) = *self.debug_log_path.read().await {
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log_path) {
                    use std::io::Write;
                    let _ = writeln!(f, "[{}] [SubscriptionHub] Sending initial snapshot for subscription {}", 
                        chrono::Local::now().format("%H:%M:%S%.3f"), subscription_id);
                }
            }
            
            let snapshot = source.snapshot(config.dimensions).await?;
            self.send_to_subscription(&subscription, ServerMessage::Snapshot {
                subscription_id: subscription_id.clone(),
                sequence: 0,
                grid: snapshot,
                timestamp: chrono::Utc::now().timestamp(),
                checksum: 0,
            }).await?;
            
            if let Some(ref debug_log_path) = *self.debug_log_path.read().await {
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log_path) {
                    use std::io::Write;
                    let _ = writeln!(f, "[{}] [SubscriptionHub] Snapshot sent successfully for {}", 
                        chrono::Local::now().format("%H:%M:%S%.3f"), subscription_id);
                }
            }
        }
        
        // Store subscription
        {
            let mut subscriptions = self.subscriptions.write().await;
            subscriptions.insert(subscription_id.clone(), subscription);
        }
        
        // Track client subscriptions
        {
            let mut clients = self.clients.write().await;
            clients.entry(client_id).or_insert_with(Vec::new).push(subscription_id.clone());
        }
        
        // Notify handler
        if let Some(handler) = self.handler.read().await.as_ref() {
            handler.on_subscribe(&subscription_id, &config).await?;
        }
        
        Ok(subscription_id)
    }

    /// Create a new subscription with a provided ID (from client)
    pub async fn subscribe_with_id(
        &self,
        client_id: ClientId,
        client_transport: Arc<dyn Transport>,
        subscription_id: SubscriptionId,
        config: SubscriptionConfig,
    ) -> Result<()> {
        // Debug: print subscribe call
        if let Some(ref debug_log_path) = *self.debug_log_path.read().await {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log_path) {
                use std::io::Write;
                let _ = writeln!(f, "[{}] [SubscriptionHub] subscribe_with_id called: client={}, subscription={}", 
                    chrono::Local::now().format("%H:%M:%S%.3f"), client_id, subscription_id);
            }
        }
        
        let subscription = Subscription {
            id: subscription_id.clone(),
            client_id: client_id.clone(),
            dimensions: config.dimensions.clone(),
            mode: config.mode.clone(),
            position: config.position.clone(),
            transport: client_transport.clone(),
            is_controlling: config.is_controlling,
            last_sequence_acked: 0,
            current_sequence: 0,
        };

        // Send subscription acknowledgment
        self.send_to_subscription(&subscription, ServerMessage::SubscriptionAck {
            subscription_id: subscription_id.clone(),
            status: SubscriptionStatus::Active,
            shared_with: None,
        }).await?;

        // Send initial snapshot if we have a data source
        if let Some(source) = self.terminal_source.read().await.as_ref() {
            if let Some(ref debug_log_path) = *self.debug_log_path.read().await {
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log_path) {
                    use std::io::Write;
                    let _ = writeln!(f, "[{}] [SubscriptionHub] Sending initial snapshot for subscription {}", 
                        chrono::Local::now().format("%H:%M:%S%.3f"), subscription_id);
                }
            }
            
            let snapshot = source.snapshot(config.dimensions).await?;
            self.send_to_subscription(&subscription, ServerMessage::Snapshot {
                subscription_id: subscription_id.clone(),
                sequence: 0,
                grid: snapshot,
                timestamp: chrono::Utc::now().timestamp(),
                checksum: 0,
            }).await?;
            
            if let Some(ref debug_log_path) = *self.debug_log_path.read().await {
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(debug_log_path) {
                    use std::io::Write;
                    let _ = writeln!(f, "[{}] [SubscriptionHub] Snapshot sent successfully for {}", 
                        chrono::Local::now().format("%H:%M:%S%.3f"), subscription_id);
                }
            }
        }

        // Store subscription
        {
            let mut subscriptions = self.subscriptions.write().await;
            subscriptions.insert(subscription_id.clone(), subscription);
        }

        // Track client subscriptions
        {
            let mut clients = self.clients.write().await;
            clients.entry(client_id).or_insert_with(Vec::new).push(subscription_id.clone());
        }

        // Notify handler
        if let Some(handler) = self.handler.read().await.as_ref() {
            handler.on_subscribe(&subscription_id, &config).await?;
        }

        Ok(())
    }
    
    /// Update an existing subscription
    pub async fn update(&self, id: &SubscriptionId, patch: SubscriptionUpdate) -> Result<()> {
        let mut subscriptions = self.subscriptions.write().await;
        let subscription = subscriptions.get_mut(id)
            .ok_or_else(|| anyhow!("Subscription not found"))?;
        
        let dimensions_changed = patch.dimensions.is_some();
            
        if let Some(dims) = patch.dimensions {
            subscription.dimensions = dims;
        }
        if let Some(mode) = patch.mode {
            subscription.mode = mode;
        }
        if let Some(pos) = patch.position {
            subscription.position = Some(pos);
        }
        if let Some(ctrl) = patch.is_controlling {
            subscription.is_controlling = ctrl;
        }
        
        // Send updated snapshot if dimensions changed
        if dimensions_changed {
            if let Some(source) = self.terminal_source.read().await.as_ref() {
                let snapshot = source.snapshot(subscription.dimensions.clone()).await?;
                subscription.current_sequence += 1;
                self.send_to_subscription(subscription, ServerMessage::Snapshot {
                    subscription_id: id.clone(),
                    sequence: subscription.current_sequence,
                    grid: snapshot,
                    timestamp: chrono::Utc::now().timestamp(),
                    checksum: 0,
                }).await?;
            }
        }
        
        Ok(())
    }
    
    /// Remove a subscription
    pub async fn unsubscribe(&self, id: &SubscriptionId) -> Result<()> {
        // Notify handler first
        if let Some(handler) = self.handler.read().await.as_ref() {
            handler.on_unsubscribe(id).await?;
        }
        
        let mut subscriptions = self.subscriptions.write().await;
        if let Some(subscription) = subscriptions.remove(id) {
            // Remove from client tracking
            let mut clients = self.clients.write().await;
            if let Some(client_subs) = clients.get_mut(&subscription.client_id) {
                client_subs.retain(|sid| sid != id);
                if client_subs.is_empty() {
                    clients.remove(&subscription.client_id);
                }
            }
        }
        
        Ok(())
    }
    
    /// Start event-driven streaming (server-only)
    /// Returns a JoinHandle for the streaming task
    pub fn start_streaming(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let (tx, mut rx) = mpsc::channel::<()>(1);
        
        // Store the sender for stopping the task later
        let hub = self.clone();
        tokio::spawn(async move {
            *hub.delta_tx.write().await = Some(tx);
        });
        
        // Start the streaming task
        tokio::spawn(async move {
            // Log streaming task start
            if let Some(ref path) = *self.debug_log_path.read().await {
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                    use std::io::Write;
                    let _ = writeln!(f, "[{}] [SubscriptionHub] Delta streaming task started",
                        chrono::Local::now().format("%H:%M:%S%.3f"));
                }
            }
            
            let mut iteration = 0;
            loop {
                iteration += 1;
                // Check if we should stop
                if rx.try_recv().is_ok() {
                    break;
                }
                
                // Get the terminal source
                let source = {
                    let guard = self.terminal_source.read().await;
                    guard.as_ref().map(|s| s.clone())
                };
                
                if let Some(source) = source {
                    // Log every 10th iteration to avoid spam
                    if iteration % 10 == 0 {
                        if let Some(ref path) = *self.debug_log_path.read().await {
                            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                                use std::io::Write;
                                let _ = writeln!(f, "[{}] [SubscriptionHub] Streaming loop iteration {}, waiting for delta...",
                                    chrono::Local::now().format("%H:%M:%S%.3f"), iteration);
                            }
                        }
                    }
                    
                    // Wait for next delta
                    match source.next_delta().await {
                        Ok(delta) => {
                            // Log delta received
                            if let Some(ref path) = *self.debug_log_path.read().await {
                                if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                                    use std::io::Write;
                                    let _ = writeln!(f, "[{}] [SubscriptionHub] Received delta: {} cell changes, cursor: {:?}, dim: {:?}",
                                        chrono::Local::now().format("%H:%M:%S%.3f"), 
                                        delta.cell_changes.len(),
                                        delta.cursor_change.is_some(),
                                        delta.dimension_change.is_some());
                                }
                            }
                            
                            // Broadcast delta to all subscriptions
                            let _ = self.push_delta(delta).await;
                        }
                        Err(e) => {
                            // Log error
                            if let Some(ref path) = *self.debug_log_path.read().await {
                                if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                                    use std::io::Write;
                                    let _ = writeln!(f, "[{}] [SubscriptionHub] Error getting delta: {:?}",
                                        chrono::Local::now().format("%H:%M:%S%.3f"), e);
                                }
                            }
                            
                            // Error getting delta, wait a bit before retrying
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        }
                    }
                } else {
                    // No source attached yet, wait
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        })
    }
    
    /// Push a delta to all subscriptions
    pub async fn push_delta(&self, delta: crate::server::terminal_state::GridDelta) -> Result<()> {
        // Update per-subscription sequence numbers under a write lock
        let mut subscriptions = self.subscriptions.write().await;
        
        // Log number of active subscriptions
        if let Some(ref path) = *self.debug_log_path.read().await {
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                use std::io::Write;
                let _ = writeln!(f, "[{}] [SubscriptionHub] Broadcasting delta to {} subscriptions",
                    chrono::Local::now().format("%H:%M:%S%.3f"), subscriptions.len());
            }
        }
        
        for subscription in subscriptions.values_mut() {
            subscription.current_sequence = subscription.current_sequence.saturating_add(1);
            let msg = ServerMessage::Delta {
                subscription_id: subscription.id.clone(),
                sequence: subscription.current_sequence,
                changes: delta.clone(),
                timestamp: chrono::Utc::now().timestamp(),
            };
            
            // Log sending delta to each subscription
            if let Some(ref path) = *self.debug_log_path.read().await {
                if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                    use std::io::Write;
                    let _ = writeln!(f, "[{}] [SubscriptionHub] Sending Delta to sub {} (seq {})",
                        chrono::Local::now().format("%H:%M:%S%.3f"), subscription.id, subscription.current_sequence);
                }
            }
            
            let _ = self.send_to_subscription(subscription, msg).await;
        }
        Ok(())
    }
    
    /// Force a snapshot for a specific subscription
    pub async fn force_snapshot(&self, id: &SubscriptionId) -> Result<()> {
        let mut subscriptions = self.subscriptions.write().await;
        let subscription = subscriptions.get_mut(id)
            .ok_or_else(|| anyhow!("Subscription not found"))?;
            
        if let Some(source) = self.terminal_source.read().await.as_ref() {
            let snapshot = source.snapshot(subscription.dimensions.clone()).await?;
            subscription.current_sequence = subscription.current_sequence.saturating_add(1);
            let sequence = subscription.current_sequence;
            self.send_to_subscription(subscription, ServerMessage::Snapshot {
                subscription_id: id.clone(),
                sequence,
                grid: snapshot,
                timestamp: chrono::Utc::now().timestamp(),
                checksum: 0,
            }).await?;
        }
        
        Ok(())
    }
    
    /// Handle incoming client message for a subscription
    pub async fn handle_incoming(&self, client_id: &ClientId, msg: ClientMessage) -> Result<()> {
        match msg {
            ClientMessage::Subscribe { subscription_id, dimensions, mode, position, .. } => {
                // Subscribe requires access to the client's transport; handle at server level
                // This function is transport-agnostic, so we document that server should call
                // subscribe_with_id() directly when a Subscribe is received.
                // No-op here to avoid partial subscriptions without transport.
            }
            ClientMessage::TerminalInput { data, .. } => {
                // Debug log: Terminal input received
                if let Some(ref path) = *self.debug_log_path.read().await {
                    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                        use std::io::Write;
                        let _ = writeln!(f, "[{}] [SubscriptionHub] Received TerminalInput from client {}: {} bytes",
                            chrono::Local::now().format("%H:%M:%S%.3f"), client_id, data.len());
                    }
                }
                
                // Check if any subscription for this client is controlling
                let subscriptions = self.subscriptions.read().await;
                let controlling_sub = subscriptions.values()
                    .find(|s| s.client_id == *client_id && s.is_controlling);
                let has_any_sub = subscriptions.values()
                    .any(|s| s.client_id == *client_id);
                    
                // Debug log: Subscription control state
                if let Some(ref path) = *self.debug_log_path.read().await {
                    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                        use std::io::Write;
                        let _ = writeln!(f, "[{}] [SubscriptionHub] Client {} subscription state: has_any={}, is_controlling={}",
                            chrono::Local::now().format("%H:%M:%S%.3f"), 
                            client_id, 
                            has_any_sub,
                            controlling_sub.is_some());
                        if let Some(sub) = controlling_sub {
                            let _ = writeln!(f, "[{}] [SubscriptionHub] Controlling subscription: id={}", 
                                chrono::Local::now().format("%H:%M:%S%.3f"), sub.id);
                        }
                    }
                }
                    
                if let Some(sub) = controlling_sub {
                    // Forward to PTY writer if available
                    if let Some(writer) = self.pty_writer.read().await.as_ref() {
                        // Debug log: Before PTY write
                        let write_start = std::time::Instant::now();
                        if let Some(ref path) = *self.debug_log_path.read().await {
                            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                                use std::io::Write;
                                let _ = writeln!(f, "[{}] [SubscriptionHub] Forwarding {} bytes to PTY writer",
                                    chrono::Local::now().format("%H:%M:%S%.3f"), data.len());
                            }
                        }
                        
                        let write_result = writer.write(&data).await;
                        
                        // Debug log: After PTY write
                        if let Some(ref path) = *self.debug_log_path.read().await {
                            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                                use std::io::Write;
                                let elapsed = write_start.elapsed();
                                match &write_result {
                                    Ok(_) => {
                                        let _ = writeln!(f, "[{}] [SubscriptionHub] PTY write successful, took {}ms",
                                            chrono::Local::now().format("%H:%M:%S%.3f"), elapsed.as_millis());
                                    },
                                    Err(e) => {
                                        let _ = writeln!(f, "[{}] [SubscriptionHub] PTY write failed after {}ms: {:?}",
                                            chrono::Local::now().format("%H:%M:%S%.3f"), elapsed.as_millis(), e);
                                    }
                                }
                            }
                        }
                        
                        write_result?;
                    } else {
                        // Debug log: No PTY writer
                        if let Some(ref path) = *self.debug_log_path.read().await {
                            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                                use std::io::Write;
                                let _ = writeln!(f, "[{}] [SubscriptionHub] WARNING: No PTY writer available",
                                    chrono::Local::now().format("%H:%M:%S%.3f"));
                            }
                        }
                    }
                    
                    // Notify handler
                    if let Some(handler) = self.handler.read().await.as_ref() {
                        handler.on_input(&sub.id, data).await?;
                    }
                } else {
                    // Debug log: Input not allowed
                    if let Some(ref path) = *self.debug_log_path.read().await {
                        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                            use std::io::Write;
                            let _ = writeln!(f, "[{}] [SubscriptionHub] INPUT_NOT_ALLOWED: Client {} does not have controlling subscription",
                                chrono::Local::now().format("%H:%M:%S%.3f"), client_id);
                        }
                    }
                    
                    // Send error to client
                    if let Some(subscription) = subscriptions.values()
                        .find(|s| s.client_id == *client_id) {
                        self.send_to_subscription(subscription, ServerMessage::Error {
                            subscription_id: Some(subscription.id.clone()),
                            code: ErrorCode::INPUT_NOT_ALLOWED,
                            message: "Client does not have input permissions".to_string(),
                            recoverable: true,
                            retry_after: None,
                        }).await?;
                    }
                }
            }
            
            ClientMessage::ModifySubscription { subscription_id, dimensions, mode, position } => {
                // Clone dimensions before moving into update
                let dims_for_handler = dimensions.clone();
                
                let update = SubscriptionUpdate {
                    dimensions,
                    mode,
                    position,
                    ..Default::default()
                };
                self.update(&subscription_id, update).await?;
                
                // Notify handler about resize if dimensions changed
                if let Some(dims) = dims_for_handler {
                    if let Some(handler) = self.handler.read().await.as_ref() {
                        handler.on_resize(&subscription_id, dims).await?;
                    }
                }
            }
            
            ClientMessage::Unsubscribe { subscription_id } => {
                self.unsubscribe(&subscription_id).await?;
            }
            
            ClientMessage::RequestState { subscription_id, .. } => {
                self.force_snapshot(&subscription_id).await?;
            }
            
            _ => {
                // Other messages handled elsewhere
            }
        }
        
        Ok(())
    }
    
    /// Send a message to a specific subscription with channel routing
    async fn send_to_subscription(&self, subscription: &Subscription, message: ServerMessage) -> Result<()> {
        // Log what we're about to send
        if let Some(ref path) = *self.debug_log_path.read().await {
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                use std::io::Write;
                match &message {
                    ServerMessage::Snapshot { subscription_id, .. } => {
                        let _ = writeln!(f, "[{}] [SubscriptionHub] send_to_subscription: Sending Snapshot to {}",
                            chrono::Local::now().format("%H:%M:%S%.3f"), subscription_id);
                    }
                    ServerMessage::Delta { subscription_id, sequence, .. } => {
                        let _ = writeln!(f, "[{}] [SubscriptionHub] send_to_subscription: Sending Delta seq {} to {}",
                            chrono::Local::now().format("%H:%M:%S%.3f"), sequence, subscription_id);
                    }
                    ServerMessage::SubscriptionAck { subscription_id, .. } => {
                        let _ = writeln!(f, "[{}] [SubscriptionHub] send_to_subscription: Sending SubscriptionAck to {}",
                            chrono::Local::now().format("%H:%M:%S%.3f"), subscription_id);
                    }
                    _ => {}
                }
            }
        }
        
        // Wrap in AppMessage::Protocol for client demux path
        let app_envelope = crate::protocol::signaling::AppMessage::Protocol {
            message: serde_json::to_value(&message)?,
        };
        let bytes = serde_json::to_vec(&app_envelope)?;
        
        // Send via transport
        let send_result = subscription.transport.send(&bytes).await;
        
        // Log send result
        if let Some(ref path) = *self.debug_log_path.read().await {
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                use std::io::Write;
                match &send_result {
                    Ok(_) => {
                        let _ = writeln!(f, "[{}] [SubscriptionHub] send_to_subscription: Successfully sent message",
                            chrono::Local::now().format("%H:%M:%S%.3f"));
                    }
                    Err(e) => {
                        let _ = writeln!(f, "[{}] [SubscriptionHub] send_to_subscription: Failed to send: {:?}",
                            chrono::Local::now().format("%H:%M:%S%.3f"), e);
                    }
                }
            }
        }
        
        send_result?;
        Ok(())
    }
    
    /// Remove all subscriptions for a client
    pub async fn remove_client(&self, client_id: &ClientId) -> Result<()> {
        let mut clients = self.clients.write().await;
        if let Some(subscription_ids) = clients.remove(client_id) {
            let mut subscriptions = self.subscriptions.write().await;
            
            // Notify handler for each subscription
            if let Some(handler) = self.handler.read().await.as_ref() {
                for id in &subscription_ids {
                    let _ = handler.on_unsubscribe(id).await;
                }
            }
            
            // Remove all subscriptions
            for id in subscription_ids {
                subscriptions.remove(&id);
            }
        }
        Ok(())
    }
}

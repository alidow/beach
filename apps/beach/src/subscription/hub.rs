use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{RwLock, mpsc};
use anyhow::{Result, anyhow};
use async_trait::async_trait;

use crate::protocol::{
    ClientMessage, ServerMessage, ViewMode, ViewPosition, Dimensions,
    ErrorCode, SubscriptionStatus
};
use crate::transport::Transport;
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
    /// Last grid sent to this subscription (for delta computation)
    previous_grid: Option<crate::server::terminal_state::Grid>,
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
    
    // Debug recorder for structured logging
    debug_recorder: Arc<RwLock<Option<Arc<Mutex<crate::debug_recorder::DebugRecorder>>>>>,
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
            debug_recorder: Arc::new(RwLock::new(None)),
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
    
    /// Set the debug recorder for structured logging
    pub async fn set_debug_recorder(&self, recorder: Arc<Mutex<crate::debug_recorder::DebugRecorder>>) {
        *self.debug_recorder.write().await = Some(recorder);
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
        
        let mut subscription = Subscription {
            id: subscription_id.clone(),
            client_id: client_id.clone(),
            dimensions: config.dimensions.clone(),
            mode: config.mode.clone(),
            position: config.position.clone(),
            transport: client_transport.clone(),
            is_controlling: config.is_controlling,
            last_sequence_acked: 0,
            current_sequence: 0,
            previous_grid: None,
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
            
            // Use snapshot_with_view to support different view modes from the start
            let snapshot = source.snapshot_with_view(
                config.dimensions,
                config.mode.clone(),
                config.position.clone()
            ).await?;
            
            // Store initial grid for future delta computation
            subscription.previous_grid = Some(snapshot.clone());
            
            // Log snapshot to debug recorder if available (non-blocking)
            if let Some(ref recorder) = *self.debug_recorder.read().await {
                if let Ok(mut rec) = recorder.try_lock() {
                    let _ = rec.record_server_subscription_snapshot(
                        &subscription_id,
                        0,
                        &snapshot
                    );
                    let _ = rec.record_grid_bottom_context("server_subscribe_with_id.initial_snapshot", &snapshot, 6);
                }
            }
            
            self.send_to_subscription(&subscription, ServerMessage::Snapshot {
                subscription_id: subscription_id.clone(),
                sequence: 0,
                grid: snapshot,
                timestamp: chrono::Utc::now().timestamp(),
                checksum: 0,
            }).await?;
            
            // Send history metadata if available
            if let Ok(metadata) = source.get_history_metadata().await {
                self.send_to_subscription(&subscription, ServerMessage::HistoryInfo {
                    subscription_id: subscription_id.clone(),
                    oldest_line: metadata.oldest_line,
                    latest_line: metadata.latest_line,
                    total_lines: metadata.total_lines,
                    oldest_timestamp: metadata.oldest_timestamp.map(|dt| dt.timestamp()),
                    latest_timestamp: metadata.latest_timestamp.map(|dt| dt.timestamp()),
                }).await?;
            }
            
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
        
        let mut subscription = Subscription {
            id: subscription_id.clone(),
            client_id: client_id.clone(),
            dimensions: config.dimensions.clone(),
            mode: config.mode.clone(),
            position: config.position.clone(),
            transport: client_transport.clone(),
            is_controlling: config.is_controlling,
            last_sequence_acked: 0,
            current_sequence: 0,
            previous_grid: None,
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
            
            // Use snapshot_with_view to support different view modes from the start
            let snapshot = source.snapshot_with_view(
                config.dimensions,
                config.mode.clone(),
                config.position.clone()
            ).await?;
            
            // Store initial grid for future delta computation
            subscription.previous_grid = Some(snapshot.clone());
            
            // Log snapshot to debug recorder if available (non-blocking)
            if let Some(ref recorder) = *self.debug_recorder.read().await {
                if let Ok(mut rec) = recorder.try_lock() {
                    let _ = rec.record_server_subscription_snapshot(
                        &subscription_id,
                        0,
                        &snapshot
                    );
                }
            }
            
            self.send_to_subscription(&subscription, ServerMessage::Snapshot {
                subscription_id: subscription_id.clone(),
                sequence: 0,
                grid: snapshot,
                timestamp: chrono::Utc::now().timestamp(),
                checksum: 0,
            }).await?;
            
            // Send history metadata if available
            if let Ok(metadata) = source.get_history_metadata().await {
                self.send_to_subscription(&subscription, ServerMessage::HistoryInfo {
                    subscription_id: subscription_id.clone(),
                    oldest_line: metadata.oldest_line,
                    latest_line: metadata.latest_line,
                    total_lines: metadata.total_lines,
                    oldest_timestamp: metadata.oldest_timestamp.map(|dt| dt.timestamp()),
                    latest_timestamp: metadata.latest_timestamp.map(|dt| dt.timestamp()),
                }).await?;
            }
            
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
        let view_changed = patch.mode.is_some() || patch.position.is_some();
            
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
        
        // Send updated snapshot if dimensions or view changed
        if dimensions_changed || view_changed {
            // Debug log the modification
            if let Ok(mut debug_file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/beach-subscription.log")
            {
                use std::io::Write;
                let _ = writeln!(debug_file, 
                    "[{}] ModifySubscription: id={}, dims_changed={}, view_changed={}, mode={:?}, position={:?}",
                    chrono::Utc::now(),
                    id,
                    dimensions_changed,
                    view_changed,
                    subscription.mode,
                    subscription.position.as_ref().map(|p| format!("line={:?}", p.line))
                );
            }
            
            if let Some(source) = self.terminal_source.read().await.as_ref() {
                // Use the new snapshot_with_view method to support historical views
                let snapshot = source.snapshot_with_view(
                    subscription.dimensions.clone(),
                    subscription.mode.clone(),
                    subscription.position.clone()
                ).await?;
                
                subscription.current_sequence += 1;
                
                // Debug log the snapshot being sent
                if let Ok(mut debug_file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("/tmp/beach-subscription.log")
                {
                    use std::io::Write;
                    let _ = writeln!(debug_file, 
                        "[{}] SENDING SNAPSHOT: id={}, seq={}, grid_dims={}x{}, start_line={:?}, end_line={:?}",
                        chrono::Utc::now(),
                        id,
                        subscription.current_sequence,
                        snapshot.width, snapshot.height,
                        snapshot.start_line.to_u64(),
                        snapshot.end_line.to_u64()
                    );
                }
                
                // Reset previous_grid when dimensions or view changes
                subscription.previous_grid = Some(snapshot.clone());
                
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
                            
                            // Push terminal updates with per-subscription deltas
                            let _ = self.push_terminal_update().await;
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
    
    /// Push terminal updates to all subscriptions with per-subscription deltas
    pub async fn push_terminal_update(&self) -> Result<()> {
        use crate::server::terminal_state::GridDelta;
        
        // Get terminal source
        let source = self.terminal_source.read().await;
        let source = source.as_ref().ok_or_else(|| anyhow!("No terminal source"))?;
        
        // Update each subscription with its own delta
        let mut subscriptions = self.subscriptions.write().await;
        
        for subscription in subscriptions.values_mut() {
            // Get current snapshot for this subscription's dimensions and view mode
            let current_snapshot = source.snapshot_with_view(
                subscription.dimensions.clone(),
                subscription.mode.clone(),
                subscription.position.clone()
            ).await?;
            
            // Compute delta if we have a previous grid
            if let Some(ref previous_grid) = subscription.previous_grid {
                // Generate delta between previous and current
                let delta = GridDelta::diff(previous_grid, &current_snapshot);
                
                // Only send if there are actual changes
                if !delta.cell_changes.is_empty() || 
                   delta.dimension_change.is_some() || 
                   delta.cursor_change.is_some() {
                    
                    subscription.current_sequence = subscription.current_sequence.saturating_add(1);
                    
                    // Log delta to debug recorder
                    if let Some(ref recorder) = *self.debug_recorder.read().await {
                        let modified_lines: Vec<u16> = delta.cell_changes.iter()
                            .map(|change| change.row)
                            .collect::<std::collections::HashSet<_>>()
                            .into_iter()
                            .collect();
                        
                        if let Ok(mut rec) = recorder.try_lock() {
                            let _ = rec.record_event(
                                crate::debug_recorder::DebugEvent::ServerSubscriptionDelta {
                                    timestamp: chrono::Utc::now(),
                                    subscription_id: subscription.id.clone(),
                                    sequence: subscription.current_sequence,
                                    cell_changes_count: delta.cell_changes.len(),
                                    has_dimension_change: delta.dimension_change.is_some(),
                                    has_cursor_change: delta.cursor_change.is_some(),
                                    modified_lines,
                                }
                            );
                            let _ = rec.record_grid_bottom_context("server_push_terminal_update.current_snapshot", &current_snapshot, 6);
                            // Also record a seam context window around the first modified row
                            if let Some(min_row) = delta.cell_changes.iter().map(|c| c.row).min() {
                                let start = min_row.saturating_sub(2);
                                let end = (min_row.saturating_add(6)).min(current_snapshot.height.saturating_sub(1));
                                let mut before_lines = Vec::new();
                                let mut after_lines = Vec::new();
                                if let Some(prev) = subscription.previous_grid.as_ref() {
                                    for row in start..=end {
                                        let mut line = String::new();
                                        for col in 0..prev.width.min(120) {
                                            if let Some(cell) = prev.get_cell(row, col) { line.push(cell.char); }
                                        }
                                        before_lines.push((row, line.trim_end().to_string()));
                                    }
                                }
                                for row in start..=end {
                                    let mut line = String::new();
                                    for col in 0..current_snapshot.width.min(120) {
                                        if let Some(cell) = current_snapshot.get_cell(row, col) { line.push(cell.char); }
                                    }
                                    after_lines.push((row, line.trim_end().to_string()));
                                }
                                let _ = rec.record_server_delta_context(
                                    &subscription.id,
                                    subscription.current_sequence,
                                    start,
                                    &before_lines,
                                    &after_lines,
                                );
                            }
                        }
                    }
                    
                    let msg = ServerMessage::Delta {
                        subscription_id: subscription.id.clone(),
                        sequence: subscription.current_sequence,
                        changes: delta,
                        timestamp: chrono::Utc::now().timestamp(),
                    };
                    
                    // Log sending delta
                    if let Some(ref path) = *self.debug_log_path.read().await {
                        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                            use std::io::Write;
                            let _ = writeln!(f, "[{}] [SubscriptionHub] Sending custom delta to sub {} (seq {})",
                                chrono::Local::now().format("%H:%M:%S%.3f"), subscription.id, subscription.current_sequence);
                            // Optional seam debug logging
                            if std::env::var("BEACH_SEAM_DEBUG").ok().is_some() {
                                // Count trailing blank rows
                                let mut trailing_blanks = 0usize;
                                for row in (0..current_snapshot.height).rev() {
                                    let is_blank = (0..current_snapshot.width).all(|col| {
                                        current_snapshot.get_cell(row, col)
                                            .map(|c| c.char == ' ' || c.char == '\0')
                                            .unwrap_or(true)
                                    });
                                    if is_blank { trailing_blanks += 1; } else { break; }
                                }
                                let _ = writeln!(f, "[{}] [SubscriptionHub] BottomContext sub {} dims {}x{} trailing_blanks {}",
                                    chrono::Local::now().format("%H:%M:%S%.3f"),
                                    subscription.id,
                                    current_snapshot.width,
                                    current_snapshot.height,
                                    trailing_blanks);
                                // Log last up to 5 rows
                                let start = current_snapshot.height.saturating_sub(5);
                                for row in start..current_snapshot.height {
                                    let mut line = String::new();
                                    for col in 0..current_snapshot.width.min(100) {
                                        if let Some(cell) = current_snapshot.get_cell(row, col) { line.push(cell.char); }
                                    }
                                    let _ = writeln!(f, "[{}] [SubscriptionHub]   Row {}: '{}'",
                                        chrono::Local::now().format("%H:%M:%S%.3f"), row, line.trim_end());
                                }
                            }
                        }
                    }
                    
                    let _ = self.send_to_subscription(subscription, msg).await;
                }
            } else {
                // No previous grid, send initial snapshot
                subscription.current_sequence = subscription.current_sequence.saturating_add(1);
                
                // Log snapshot to debug recorder
                if let Some(ref recorder) = *self.debug_recorder.read().await {
                    let blank_line_count = current_snapshot.cells.iter()
                        .filter(|row| row.is_empty() || row.iter().all(|cell| cell.char == ' '))
                        .count();
                    let non_blank_lines = current_snapshot.cells.len() - blank_line_count;
                    
                    let content_sample: Vec<String> = current_snapshot.cells.iter()
                        .enumerate()
                        .filter(|(_, row)| !row.is_empty() && !row.iter().all(|cell| cell.char == ' '))
                        .take(10)
                        .map(|(idx, row)| {
                            format!("Line {}: {}", idx, row.iter().map(|cell| cell.char).collect::<String>().trim_end())
                        })
                        .collect();
                    
                    if let Ok(mut rec) = recorder.try_lock() {
                        let _ = rec.record_event(
                        crate::debug_recorder::DebugEvent::ServerSubscriptionSnapshot {
                            timestamp: chrono::Utc::now(),
                            subscription_id: subscription.id.clone(),
                            sequence: subscription.current_sequence,
                            dimensions: (current_snapshot.width, current_snapshot.height),
                            non_blank_lines,
                            blank_line_count,
                            content_sample,
                            cursor_info: Some((
                                current_snapshot.cursor.row,
                                current_snapshot.cursor.col,
                                current_snapshot.cursor.visible
                            )),
                        }
                        );
                    }
                    
                    // Also log the full grid for comparison with what client shows
                    if let Ok(mut rec) = recorder.try_lock() {
                        let _ = rec.record_event(
                            crate::debug_recorder::DebugEvent::ServerSubscriptionView {
                                timestamp: chrono::Utc::now(),
                                subscription_id: subscription.id.clone(),
                                grid: current_snapshot.clone(),
                                view_mode: format!("{:?}", subscription.mode),
                            }
                        );
                    }
                }
                
                // Also log bottom context to debug log (gated)
                if std::env::var("BEACH_SEAM_DEBUG").ok().is_some() {
                    if let Some(ref path) = *self.debug_log_path.read().await {
                        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                            use std::io::Write;
                            // Count trailing blanks
                            let mut trailing_blanks = 0usize;
                            for row in (0..current_snapshot.height).rev() {
                                let is_blank = (0..current_snapshot.width).all(|col| {
                                    current_snapshot.get_cell(row, col)
                                        .map(|c| c.char == ' ' || c.char == '\0')
                                        .unwrap_or(true)
                                });
                                if is_blank { trailing_blanks += 1; } else { break; }
                            }
                            let _ = writeln!(f, "[{}] [SubscriptionHub] Initial BottomContext sub {} dims {}x{} trailing_blanks {}",
                                chrono::Local::now().format("%H:%M:%S%.3f"),
                                subscription.id,
                                current_snapshot.width,
                                current_snapshot.height,
                                trailing_blanks);
                            let start = current_snapshot.height.saturating_sub(5);
                            for row in start..current_snapshot.height {
                                let mut line = String::new();
                                for col in 0..current_snapshot.width.min(100) {
                                    if let Some(cell) = current_snapshot.get_cell(row, col) { line.push(cell.char); }
                                }
                                let _ = writeln!(f, "[{}] [SubscriptionHub]   Row {}: '{}'",
                                    chrono::Local::now().format("%H:%M:%S%.3f"), row, line.trim_end());
                            }
                        }
                    }
                }

                let msg = ServerMessage::Snapshot {
                    subscription_id: subscription.id.clone(),
                    sequence: subscription.current_sequence,
                    grid: current_snapshot.clone(),
                    timestamp: chrono::Utc::now().timestamp(),
                    checksum: 0,
                };
                let _ = self.send_to_subscription(subscription, msg).await;
            }
            
            // Update previous grid for next delta computation
            subscription.previous_grid = Some(current_snapshot);
        }
        
        Ok(())
    }
    
    /// Push a delta to all subscriptions (DEPRECATED - broadcasts same delta to all)
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
            ClientMessage::Subscribe { .. } => {
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
                // Debug event: ModifySubscriptionReceived
                if let Some(recorder) = self.debug_recorder.read().await.as_ref() {
                    if let Ok(mut rec) = recorder.lock() {
                        let _ = rec.record_event(crate::debug_recorder::DebugEvent::ModifySubscriptionReceived {
                            timestamp: chrono::Utc::now(),
                            subscription_id: subscription_id.clone(),
                            mode: format!("{:?}", mode),
                            position: position.as_ref().map(|p| format!("{:?}", p)),
                        });
                    }
                }
                
                // Clone dimensions before moving into update
                let dims_for_handler = dimensions.clone();
                
                let update = SubscriptionUpdate {
                    dimensions,
                    mode,
                    position,
                    ..Default::default()
                };
                self.update(&subscription_id, update).await?;
                
                // Debug event: ModifySubscriptionProcessed
                // Get the updated subscription to log its state
                {
                    let subscriptions = self.subscriptions.read().await;
                    if let Some(sub) = subscriptions.get(&subscription_id) {
                        let mode_str = format!("{:?}", sub.mode);
                        let dims = (sub.dimensions.width, sub.dimensions.height);
                        drop(subscriptions); // Drop the lock before recording event
                        
                        if let Some(recorder) = self.debug_recorder.read().await.as_ref() {
                            if let Ok(mut rec) = recorder.lock() {
                                // Note: We'd need to get the actual grid to count blank lines
                                // For now, just record the dimensions and mode
                                let _ = rec.record_event(crate::debug_recorder::DebugEvent::ModifySubscriptionProcessed {
                                    timestamp: chrono::Utc::now(),
                                    subscription_id: subscription_id.clone(),
                                    mode: mode_str,
                                    result_grid_dims: dims,
                                    result_blank_count: 0, // TODO: Get actual blank count from grid
                                });
                            }
                        }
                    }
                }
                
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

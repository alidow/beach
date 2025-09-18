use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::sync::{Arc, Mutex};
use tokio::sync::{RwLock, mpsc};

use super::data_source::{PtyWriter, TerminalDataSource};
use super::{ClientId, SubscriptionId};
use crate::protocol::{
    ClientMessage, Dimensions, ErrorCode, ServerMessage, SubscriptionStatus, ViewMode, ViewPosition,
};
use crate::transport::Transport;

/// Configuration for a subscription
#[derive(Clone, Debug)]
pub struct SubscriptionConfig {
    pub dimensions: Dimensions,
    pub mode: ViewMode,
    pub position: Option<ViewPosition>,
    pub is_controlling: bool,
    // New unified grid fields
    pub initial_fetch_size: Option<u32>,
    pub stream_history: Option<bool>,
}

/// Updates to apply to an existing subscription
#[derive(Clone, Debug, Default)]
pub struct SubscriptionUpdate {
    pub dimensions: Option<Dimensions>,
    pub mode: Option<ViewMode>,
    pub position: Option<ViewPosition>,
    pub is_controlling: Option<bool>,
}

/// Message priority levels for queue ordering
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MessagePriority {
    /// P0: Highest priority - Immediate viewport snapshots
    ViewportSnapshot = 0,
    /// P0.5: High priority - Prefetch region snapshots for smooth scrolling
    PrefetchSnapshot = 1,
    /// P1: High priority - Delta updates within viewport region
    ViewportDelta = 2,
    /// P2: Medium priority - Background snapshot requests outside prefetch regions
    BackgroundSnapshot = 3,
    /// P3: Lowest priority - Background delta updates outside viewport
    BackgroundDelta = 4,
}

/// Regions for determining message priority
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineRegion {
    /// Line is within the immediate viewport
    Viewport,
    /// Line is within the prefetch margin around viewport
    Prefetch,
    /// Line is outside both viewport and prefetch regions
    Background,
}

/// A prioritized message for the send queue
#[derive(Debug, Clone)]
pub struct PrioritizedMessage {
    pub priority: MessagePriority,
    pub subscription_id: SubscriptionId,
    pub message: ServerMessage,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl PartialEq for PrioritizedMessage {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.timestamp == other.timestamp
    }
}

impl Eq for PrioritizedMessage {}

impl PartialOrd for PrioritizedMessage {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PrioritizedMessage {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering for BinaryHeap (max-heap) so that lower
        // priority numbers are considered "greater" and popped first.
        // Then, for equal priority, prefer OLDER timestamps first.
        other
            .priority
            .cmp(&self.priority)
            // Older (smaller) timestamp should be considered greater, so reverse compare
            .then_with(|| other.timestamp.cmp(&self.timestamp))
    }
}

/// A single subscription with its associated transport
#[derive(Clone)]
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
    /// Viewport-based subscription state
    viewport: Option<crate::protocol::subscription::messages::Viewport>,
    prefetch: Option<crate::protocol::subscription::messages::Prefetch>,
    follow_tail: bool,
    /// Watermark sequence for ordering guarantees
    watermark_seq: u64,
    /// Previous viewport for calculating scroll velocity
    previous_viewport: Option<crate::protocol::subscription::messages::Viewport>,
    /// Last viewport change timestamp for velocity calculation
    last_viewport_change: Option<chrono::DateTime<chrono::Utc>>,
    /// Calculated scroll velocity (lines per second)
    scroll_velocity: f64,
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

    // Channel for priority queue processing task (wake triggers)
    queue_tx: Arc<RwLock<Option<mpsc::Sender<()>>>>,

    // Channel for stopping priority queue processing task
    queue_stop_tx: Arc<RwLock<Option<mpsc::Sender<()>>>>,

    // Debug log path for verbose logging
    debug_log_path: Arc<RwLock<Option<String>>>,

    // Debug recorder for structured logging
    debug_recorder: Arc<RwLock<Option<Arc<Mutex<crate::debug_recorder::DebugRecorder>>>>>,

    // Priority queue for message sending with backpressure control
    send_queue: Arc<RwLock<BinaryHeap<PrioritizedMessage>>>,
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
            queue_tx: Arc::new(RwLock::new(None)),
            queue_stop_tx: Arc::new(RwLock::new(None)),
            debug_log_path: Arc::new(RwLock::new(None)),
            debug_recorder: Arc::new(RwLock::new(None)),
            send_queue: Arc::new(RwLock::new(BinaryHeap::new())),
        }
    }

    /// Check if any subscription exists for a given client
    pub async fn has_any_for_client(&self, client_id: &ClientId) -> bool {
        let clients = self.clients.read().await;
        clients
            .get(client_id)
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    }

    /// Set debug log path for verbose logging
    pub async fn set_debug_log_path(&self, path: String) {
        *self.debug_log_path.write().await = Some(path);
    }

    /// Set the debug recorder for structured logging
    pub async fn set_debug_recorder(
        &self,
        recorder: Arc<Mutex<crate::debug_recorder::DebugRecorder>>,
    ) {
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
            viewport: None,
            prefetch: None,
            follow_tail: true, // Default to following tail for realtime mode
            watermark_seq: 0,
            previous_viewport: None,
            last_viewport_change: None,
            scroll_velocity: 0.0,
        };

        // Send acknowledgment
        self.send_to_subscription(
            &subscription,
            ServerMessage::SubscriptionAck {
                subscription_id: subscription_id.clone(),
                status: SubscriptionStatus::Active,
                shared_with: None,
            },
        )
        .await?;

        // Send initial snapshot if we have a data source
        if let Some(source) = self.terminal_source.read().await.as_ref() {
            if let Some(ref debug_log_path) = *self.debug_log_path.read().await {
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(debug_log_path)
                {
                    use std::io::Write;
                    let _ = writeln!(
                        f,
                        "[{}] [SubscriptionHub] Sending initial snapshot for subscription {}",
                        chrono::Local::now().format("%H:%M:%S%.3f"),
                        subscription_id
                    );
                }
            }

            // Use snapshot_with_view to support different view modes from the start
            let snapshot = source
                .snapshot_with_view(
                    config.dimensions,
                    config.mode.clone(),
                    config.position.clone(),
                )
                .await?;

            // Store initial grid for future delta computation
            subscription.previous_grid = Some(snapshot.clone());

            // Log snapshot to debug recorder if available (non-blocking)
            if let Some(ref recorder) = *self.debug_recorder.read().await {
                if let Ok(mut rec) = recorder.try_lock() {
                    let _ = rec.record_server_subscription_snapshot(&subscription_id, 0, &snapshot);
                    let _ = rec.record_grid_bottom_context(
                        "server_subscribe_with_id.initial_snapshot",
                        &snapshot,
                        6,
                    );
                }
            }

            self.send_to_subscription(
                &subscription,
                ServerMessage::Snapshot {
                    subscription_id: subscription_id.clone(),
                    sequence: 0,
                    grid: snapshot,
                    timestamp: chrono::Utc::now().timestamp(),
                    checksum: 0,
                },
            )
            .await?;

            // Send history metadata if available
            if let Ok(metadata) = source.get_history_metadata().await {
                self.send_to_subscription(
                    &subscription,
                    ServerMessage::HistoryInfo {
                        subscription_id: subscription_id.clone(),
                        oldest_line: metadata.oldest_line,
                        latest_line: metadata.latest_line,
                        total_lines: metadata.total_lines,
                        oldest_timestamp: metadata.oldest_timestamp.map(|dt| dt.timestamp()),
                        latest_timestamp: metadata.latest_timestamp.map(|dt| dt.timestamp()),
                    },
                )
                .await?;
            }

            if let Some(ref debug_log_path) = *self.debug_log_path.read().await {
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(debug_log_path)
                {
                    use std::io::Write;
                    let _ = writeln!(
                        f,
                        "[{}] [SubscriptionHub] Snapshot sent successfully for {}",
                        chrono::Local::now().format("%H:%M:%S%.3f"),
                        subscription_id
                    );
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
            clients
                .entry(client_id)
                .or_insert_with(Vec::new)
                .push(subscription_id.clone());
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
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(debug_log_path)
            {
                use std::io::Write;
                let _ = writeln!(
                    f,
                    "[{}] [SubscriptionHub] subscribe_with_id called: client={}, subscription={}",
                    chrono::Local::now().format("%H:%M:%S%.3f"),
                    client_id,
                    subscription_id
                );
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
            viewport: None,
            prefetch: None,
            follow_tail: true, // Default to following tail for realtime mode
            watermark_seq: 0,
            previous_viewport: None,
            last_viewport_change: None,
            scroll_velocity: 0.0,
        };

        // Send subscription acknowledgment
        self.send_to_subscription(
            &subscription,
            ServerMessage::SubscriptionAck {
                subscription_id: subscription_id.clone(),
                status: SubscriptionStatus::Active,
                shared_with: None,
            },
        )
        .await?;

        // Store subscription BEFORE sending initial snapshot to avoid race condition
        {
            let mut subscriptions = self.subscriptions.write().await;
            subscriptions.insert(subscription_id.clone(), subscription.clone());
        }

        // Send initial data if we have a data source
        if let Some(source) = self.terminal_source.read().await.as_ref() {
            if let Some(ref debug_log_path) = *self.debug_log_path.read().await {
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(debug_log_path)
                {
                    use std::io::Write;
                    let _ = writeln!(
                        f,
                        "[{}] [SubscriptionHub] Sending initial data for subscription {} (unified_grid={})",
                        chrono::Local::now().format("%H:%M:%S%.3f"),
                        subscription_id,
                        config.stream_history.unwrap_or(false)
                    );
                }
            }

            // Check if client wants unified grid mode
            // Debug log the condition
            if let Some(ref path) = *self.debug_log_path.read().await {
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                {
                    use std::io::Write;
                    let _ = writeln!(
                        f,
                        "[{}] [SubscriptionHub] Checking unified grid mode: stream_history={:?} -> condition={}",
                        chrono::Local::now().format("%H:%M:%S%.3f"),
                        config.stream_history,
                        config.stream_history.unwrap_or(false)
                    );
                }
            }

            if config.stream_history.unwrap_or(false) {
                // NEW: Unified grid mode
                if let Some(ref path) = *self.debug_log_path.read().await {
                    if let Ok(mut f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                    {
                        use std::io::Write;
                        let _ = writeln!(
                            f,
                            "[{}] [SubscriptionHub] USING UNIFIED GRID MODE for {}",
                            chrono::Local::now().format("%H:%M:%S%.3f"),
                            subscription_id
                        );
                    }
                }
                self.handle_unified_grid_subscription(
                    &subscription,
                    &subscription_id,
                    source,
                    &config,
                )
                .await?;
            } else {
                // Legacy viewport-based mode
                if let Some(ref path) = *self.debug_log_path.read().await {
                    if let Ok(mut f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                    {
                        use std::io::Write;
                        let _ = writeln!(
                            f,
                            "[{}] [SubscriptionHub] USING LEGACY VIEWPORT MODE for {}",
                            chrono::Local::now().format("%H:%M:%S%.3f"),
                            subscription_id
                        );
                    }
                }
                let snapshot = source
                    .snapshot_with_view(
                        config.dimensions,
                        config.mode.clone(),
                        config.position.clone(),
                    )
                    .await?;

                // Store initial grid for future delta computation
                subscription.previous_grid = Some(snapshot.clone());

                // Log snapshot to debug recorder if available (non-blocking)
                if let Some(ref recorder) = *self.debug_recorder.read().await {
                    if let Ok(mut rec) = recorder.try_lock() {
                        let _ =
                            rec.record_server_subscription_snapshot(&subscription_id, 0, &snapshot);
                    }
                }

                self.send_to_subscription(
                    &subscription,
                    ServerMessage::Snapshot {
                        subscription_id: subscription_id.clone(),
                        sequence: 0,
                        grid: snapshot,
                        timestamp: chrono::Utc::now().timestamp(),
                        checksum: 0,
                    },
                )
                .await?;

                // Update stored subscription with the initial grid
                {
                    let mut subscriptions = self.subscriptions.write().await;
                    if let Some(stored_sub) = subscriptions.get_mut(&subscription_id) {
                        stored_sub.previous_grid = subscription.previous_grid.clone();
                    }
                }
            }

            // Send history metadata if available
            if let Ok(metadata) = source.get_history_metadata().await {
                self.send_to_subscription(
                    &subscription,
                    ServerMessage::HistoryInfo {
                        subscription_id: subscription_id.clone(),
                        oldest_line: metadata.oldest_line,
                        latest_line: metadata.latest_line,
                        total_lines: metadata.total_lines,
                        oldest_timestamp: metadata.oldest_timestamp.map(|dt| dt.timestamp()),
                        latest_timestamp: metadata.latest_timestamp.map(|dt| dt.timestamp()),
                    },
                )
                .await?;
            }

            if let Some(ref debug_log_path) = *self.debug_log_path.read().await {
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(debug_log_path)
                {
                    use std::io::Write;
                    let _ = writeln!(
                        f,
                        "[{}] [SubscriptionHub] Snapshot sent successfully for {}",
                        chrono::Local::now().format("%H:%M:%S%.3f"),
                        subscription_id
                    );
                }
            }
        }

        // Track client subscriptions
        {
            let mut clients = self.clients.write().await;
            clients
                .entry(client_id)
                .or_insert_with(Vec::new)
                .push(subscription_id.clone());
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
        let subscription = subscriptions
            .get_mut(id)
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
                let _ = writeln!(
                    debug_file,
                    "[{}] ModifySubscription: id={}, dims_changed={}, view_changed={}, mode={:?}, position={:?}",
                    chrono::Utc::now(),
                    id,
                    dimensions_changed,
                    view_changed,
                    subscription.mode,
                    subscription
                        .position
                        .as_ref()
                        .map(|p| format!("line={:?}", p.line))
                );
            }

            if let Some(source) = self.terminal_source.read().await.as_ref() {
                // Use the new snapshot_with_view method to support historical views
                let snapshot = source
                    .snapshot_with_view(
                        subscription.dimensions.clone(),
                        subscription.mode.clone(),
                        subscription.position.clone(),
                    )
                    .await?;

                subscription.current_sequence += 1;

                // Debug log the snapshot being sent
                if let Ok(mut debug_file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("/tmp/beach-subscription.log")
                {
                    use std::io::Write;
                    let _ = writeln!(
                        debug_file,
                        "[{}] SENDING SNAPSHOT: id={}, seq={}, grid_dims={}x{}, start_line={:?}, end_line={:?}",
                        chrono::Utc::now(),
                        id,
                        subscription.current_sequence,
                        snapshot.width,
                        snapshot.height,
                        snapshot.start_line.to_u64(),
                        snapshot.end_line.to_u64()
                    );
                }

                // Reset previous_grid when dimensions or view changes
                subscription.previous_grid = Some(snapshot.clone());

                self.send_to_subscription(
                    subscription,
                    ServerMessage::Snapshot {
                        subscription_id: id.clone(),
                        sequence: subscription.current_sequence,
                        grid: snapshot,
                        timestamp: chrono::Utc::now().timestamp(),
                        checksum: 0,
                    },
                )
                .await?;
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

    /// Start priority queue processing task
    pub fn start_priority_queue_processing(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let (wake_tx, mut wake_rx) = mpsc::channel::<()>(100); // Buffer for wake signals
        let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);

        // Store the senders for triggering/stopping the task later
        let hub = self.clone();
        tokio::spawn(async move {
            *hub.queue_tx.write().await = Some(wake_tx);
            *hub.queue_stop_tx.write().await = Some(stop_tx);
        });

        // Start the priority queue processing task
        tokio::spawn(async move {
            // Log task start
            if let Some(ref path) = *self.debug_log_path.read().await {
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                {
                    use std::io::Write;
                    let _ = writeln!(
                        f,
                        "[{}] [SubscriptionHub] Priority queue processing task started",
                        chrono::Local::now().format("%H:%M:%S%.3f")
                    );
                }
            }

            let mut iteration_count = 0u64;
            loop {
                // Check if we should stop
                if stop_rx.try_recv().is_ok() {
                    break;
                }

                // Check for wake signals and drain the wake channel
                let mut should_process = false;
                while wake_rx.try_recv().is_ok() {
                    should_process = true;
                }

                // Process priority queue if triggered or periodically
                if should_process {
                    let _ = self.process_priority_queue().await;
                } else {
                    // Periodic processing for any missed messages
                    let _ = self.process_priority_queue().await;
                }

                // Periodically trigger background prefetch for historical mode subscriptions
                // Do this every ~1 second (every 20 iterations at 50ms sleep)
                iteration_count += 1;
                if iteration_count % 20 == 0 {
                    // Debug: log queue length approximately once per second
                    if let Some(ref path) = *self.debug_log_path.read().await {
                        if let Ok(mut f) = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(path)
                        {
                            use std::io::Write;
                            let qlen = { self.send_queue.read().await.len() };
                            let _ = writeln!(
                                f,
                                "[{}] [SubscriptionHub] PriorityQueue heartbeat: queue_len={}",
                                chrono::Local::now().format("%H:%M:%S%.3f"),
                                qlen
                            );
                        }
                    }
                    let subscriptions = self.subscriptions.read().await;
                    for (id, sub) in subscriptions.iter() {
                        if !sub.follow_tail && sub.viewport.is_some() {
                            let _ = self.request_background_prefetch(id).await;
                        }
                    }
                }

                // Sleep briefly to avoid busy-waiting
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }

            // Log task stop
            if let Some(ref path) = *self.debug_log_path.read().await {
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                {
                    use std::io::Write;
                    let _ = writeln!(
                        f,
                        "[{}] [SubscriptionHub] Priority queue processing task stopped",
                        chrono::Local::now().format("%H:%M:%S%.3f")
                    );
                }
            }
        })
    }

    /// Start event-driven streaming (server-only)
    /// Returns a JoinHandle for the streaming task
    pub fn start_streaming(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let (tx, mut rx) = mpsc::channel::<()>(1);

        // Start priority queue processing task
        let _queue_handle = self.clone().start_priority_queue_processing();

        // Store the sender for stopping the task later
        let hub = self.clone();
        tokio::spawn(async move {
            *hub.delta_tx.write().await = Some(tx);
        });

        // Start the streaming task
        tokio::spawn(async move {
            // Log streaming task start
            if let Some(ref path) = *self.debug_log_path.read().await {
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                {
                    use std::io::Write;
                    let _ = writeln!(
                        f,
                        "[{}] [SubscriptionHub] Delta streaming task started",
                        chrono::Local::now().format("%H:%M:%S%.3f")
                    );
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
                                let _ = writeln!(
                                    f,
                                    "[{}] [SubscriptionHub] Streaming loop iteration {}, waiting for delta...",
                                    chrono::Local::now().format("%H:%M:%S%.3f"),
                                    iteration
                                );
                            }
                        }
                    }

                    // Wait for next delta
                    match source.next_delta().await {
                        Ok(delta) => {
                            // Log delta received
                            if let Some(ref path) = *self.debug_log_path.read().await {
                                if let Ok(mut f) =
                                    std::fs::OpenOptions::new().append(true).open(path)
                                {
                                    use std::io::Write;
                                    let _ = writeln!(
                                        f,
                                        "[{}] [SubscriptionHub] Received delta: {} cell changes, cursor: {:?}, dim: {:?}",
                                        chrono::Local::now().format("%H:%M:%S%.3f"),
                                        delta.cell_changes.len(),
                                        delta.cursor_change.is_some(),
                                        delta.dimension_change.is_some()
                                    );
                                }
                            }

                            // Push per-subscription updates with diffs/snapshots only
                            // Note: Raw broadcast deltas are disabled to avoid coordinate/base mismatches
                            // between global deltas and per-subscription views which can corrupt client state.
                            let _ = self.push_terminal_update().await;
                        }
                        Err(e) => {
                            // Log error
                            if let Some(ref path) = *self.debug_log_path.read().await {
                                if let Ok(mut f) =
                                    std::fs::OpenOptions::new().append(true).open(path)
                                {
                                    use std::io::Write;
                                    let _ = writeln!(
                                        f,
                                        "[{}] [SubscriptionHub] Error getting delta: {:?}",
                                        chrono::Local::now().format("%H:%M:%S%.3f"),
                                        e
                                    );
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
        let source = source
            .as_ref()
            .ok_or_else(|| anyhow!("No terminal source"))?;

        // Update each subscription with its own delta
        let mut subscriptions = self.subscriptions.write().await;

        for subscription in subscriptions.values_mut() {
            // Get current snapshot for this subscription's dimensions and view mode
            let current_snapshot = if let Some(ref viewport) = subscription.viewport {
                // Use viewport-based snapshot for deltas when viewport is active
                source
                    .snapshot_range_with_watermark(
                        subscription.dimensions.width,
                        viewport.start_line,
                        subscription.dimensions.height,
                    )
                    .await?
                    .0 // Get the Grid from (Grid, u64) tuple
            } else {
                // Fallback to legacy mode/position for compatibility
                source
                    .snapshot_with_view(
                        subscription.dimensions.clone(),
                        subscription.mode.clone(),
                        subscription.position.clone(),
                    )
                    .await?
            };

            // Compute delta if we have a previous grid
            if let Some(ref previous_grid) = subscription.previous_grid {
                // Generate delta between previous and current
                let delta = GridDelta::diff(previous_grid, &current_snapshot);

                // Only send if there are actual changes
                if !delta.cell_changes.is_empty()
                    || delta.dimension_change.is_some()
                    || delta.cursor_change.is_some()
                {
                    subscription.current_sequence = subscription.current_sequence.saturating_add(1);

                    // Log delta to debug recorder
                    if let Some(ref recorder) = *self.debug_recorder.read().await {
                        let modified_lines: Vec<u16> = delta
                            .cell_changes
                            .iter()
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
                                },
                            );
                            let _ = rec.record_grid_bottom_context(
                                "server_push_terminal_update.current_snapshot",
                                &current_snapshot,
                                6,
                            );
                            // Also record a seam context window around the first modified row
                            if let Some(min_row) = delta.cell_changes.iter().map(|c| c.row).min() {
                                let start = min_row.saturating_sub(2);
                                let end = (min_row.saturating_add(6))
                                    .min(current_snapshot.height.saturating_sub(1));
                                let mut before_lines = Vec::new();
                                let mut after_lines = Vec::new();
                                if let Some(prev) = subscription.previous_grid.as_ref() {
                                    for row in start..=end {
                                        let mut line = String::new();
                                        for col in 0..prev.width.min(120) {
                                            if let Some(cell) = prev.get_cell(row, col) {
                                                line.push(cell.char);
                                            }
                                        }
                                        before_lines.push((row, line.trim_end().to_string()));
                                    }
                                }
                                for row in start..=end {
                                    let mut line = String::new();
                                    for col in 0..current_snapshot.width.min(120) {
                                        if let Some(cell) = current_snapshot.get_cell(row, col) {
                                            line.push(cell.char);
                                        }
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

                    // Debug log delta details including viewport info
                    if let Some(ref path) = *self.debug_log_path.read().await {
                        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                            use std::io::Write;
                            let viewport_info = if let Some(ref vp) = subscription.viewport {
                                format!("viewport[{}-{}]", vp.start_line, vp.end_line)
                            } else {
                                "no_viewport".to_string()
                            };
                            let _ = writeln!(
                                f,
                                "[{}] Delta seq={} sub={} {}",
                                chrono::Local::now().format("%H:%M:%S%.3f"),
                                subscription.current_sequence,
                                subscription.id,
                                viewport_info
                            );
                            // Optional seam debug logging
                            if std::env::var("BEACH_SEAM_DEBUG").ok().is_some() {
                                // Count trailing blank rows
                                let mut trailing_blanks = 0usize;
                                for row in (0..current_snapshot.height).rev() {
                                    let is_blank = (0..current_snapshot.width).all(|col| {
                                        current_snapshot
                                            .get_cell(row, col)
                                            .map(|c| c.char == ' ' || c.char == '\0')
                                            .unwrap_or(true)
                                    });
                                    if is_blank {
                                        trailing_blanks += 1;
                                    } else {
                                        break;
                                    }
                                }
                                let _ = writeln!(
                                    f,
                                    "[{}] [SubscriptionHub] BottomContext sub {} dims {}x{} trailing_blanks {}",
                                    chrono::Local::now().format("%H:%M:%S%.3f"),
                                    subscription.id,
                                    current_snapshot.width,
                                    current_snapshot.height,
                                    trailing_blanks
                                );
                                // Log last up to 5 rows
                                let start = current_snapshot.height.saturating_sub(5);
                                for row in start..current_snapshot.height {
                                    let mut line = String::new();
                                    for col in 0..current_snapshot.width.min(100) {
                                        if let Some(cell) = current_snapshot.get_cell(row, col) {
                                            line.push(cell.char);
                                        }
                                    }
                                    let _ = writeln!(
                                        f,
                                        "[{}] [SubscriptionHub]   Row {}: '{}'",
                                        chrono::Local::now().format("%H:%M:%S%.3f"),
                                        row,
                                        line.trim_end()
                                    );
                                }
                            }
                        }
                    }

                    // TEMPORARILY DISABLED: Skip background deltas filtering for debugging
                    // TODO: Re-enable once basic synchronization is working
                    // if let Some(_viewport) = &subscription.viewport {
                    //     if !subscription.follow_tail {
                    //         let priority = self.determine_message_priority(subscription, &msg);
                    //         if matches!(priority, MessagePriority::BackgroundDelta | MessagePriority::BackgroundSnapshot) {
                    //             continue;  // Skip this delta entirely
                    //         }
                    //     }
                    // }

                    // Debug: log enqueue of delta before calling send_to_subscription
                    if let Some(ref path) = *self.debug_log_path.read().await {
                        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                            use std::io::Write;
                            let _ = writeln!(
                                f,
                                "[{}] [SubscriptionHub] push_terminal_update: enqueue Delta to sub {} (seq {})",
                                chrono::Local::now().format("%H:%M:%S%.3f"),
                                subscription.id,
                                subscription.current_sequence
                            );
                        }
                    }
                    let _ = self.send_to_subscription(subscription, msg).await;
                }
            } else {
                // No previous grid, send initial snapshot
                subscription.current_sequence = subscription.current_sequence.saturating_add(1);

                // Log snapshot to debug recorder
                if let Some(ref recorder) = *self.debug_recorder.read().await {
                    let blank_line_count = current_snapshot
                        .cells
                        .iter()
                        .filter(|row| row.is_empty() || row.iter().all(|cell| cell.char == ' '))
                        .count();
                    let non_blank_lines = current_snapshot.cells.len() - blank_line_count;

                    let content_sample: Vec<String> = current_snapshot
                        .cells
                        .iter()
                        .enumerate()
                        .filter(|(_, row)| {
                            !row.is_empty() && !row.iter().all(|cell| cell.char == ' ')
                        })
                        .take(10)
                        .map(|(idx, row)| {
                            format!(
                                "Line {}: {}",
                                idx,
                                row.iter()
                                    .map(|cell| cell.char)
                                    .collect::<String>()
                                    .trim_end()
                            )
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
                                    current_snapshot.cursor.visible,
                                )),
                            },
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
                            },
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
                                    current_snapshot
                                        .get_cell(row, col)
                                        .map(|c| c.char == ' ' || c.char == '\0')
                                        .unwrap_or(true)
                                });
                                if is_blank {
                                    trailing_blanks += 1;
                                } else {
                                    break;
                                }
                            }
                            let _ = writeln!(
                                f,
                                "[{}] [SubscriptionHub] Initial BottomContext sub {} dims {}x{} trailing_blanks {}",
                                chrono::Local::now().format("%H:%M:%S%.3f"),
                                subscription.id,
                                current_snapshot.width,
                                current_snapshot.height,
                                trailing_blanks
                            );
                            let start = current_snapshot.height.saturating_sub(5);
                            for row in start..current_snapshot.height {
                                let mut line = String::new();
                                for col in 0..current_snapshot.width.min(100) {
                                    if let Some(cell) = current_snapshot.get_cell(row, col) {
                                        line.push(cell.char);
                                    }
                                }
                                let _ = writeln!(
                                    f,
                                    "[{}] [SubscriptionHub]   Row {}: '{}'",
                                    chrono::Local::now().format("%H:%M:%S%.3f"),
                                    row,
                                    line.trim_end()
                                );
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
                let _ = writeln!(
                    f,
                    "[{}] [SubscriptionHub] Broadcasting delta to {} subscriptions",
                    chrono::Local::now().format("%H:%M:%S%.3f"),
                    subscriptions.len()
                );
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
                    let _ = writeln!(
                        f,
                        "[{}] [SubscriptionHub] Sending Delta to sub {} (seq {})",
                        chrono::Local::now().format("%H:%M:%S%.3f"),
                        subscription.id,
                        subscription.current_sequence
                    );
                }
            }

            let _ = self.send_to_subscription(subscription, msg).await;
        }
        Ok(())
    }

    /// Force a snapshot for a specific subscription
    pub async fn force_snapshot(&self, id: &SubscriptionId) -> Result<()> {
        let mut subscriptions = self.subscriptions.write().await;
        let subscription = subscriptions
            .get_mut(id)
            .ok_or_else(|| anyhow!("Subscription not found"))?;

        if let Some(source) = self.terminal_source.read().await.as_ref() {
            let snapshot = source.snapshot(subscription.dimensions.clone()).await?;
            subscription.current_sequence = subscription.current_sequence.saturating_add(1);
            let sequence = subscription.current_sequence;
            self.send_to_subscription(
                subscription,
                ServerMessage::Snapshot {
                    subscription_id: id.clone(),
                    sequence,
                    grid: snapshot,
                    timestamp: chrono::Utc::now().timestamp(),
                    checksum: 0,
                },
            )
            .await?;
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
                        let _ = writeln!(
                            f,
                            "[{}] [SubscriptionHub] Received TerminalInput from client {}: {} bytes",
                            chrono::Local::now().format("%H:%M:%S%.3f"),
                            client_id,
                            data.len()
                        );
                    }
                }

                // Check if any subscription for this client is controlling
                let subscriptions = self.subscriptions.read().await;
                let controlling_sub = subscriptions
                    .values()
                    .find(|s| s.client_id == *client_id && s.is_controlling);
                let has_any_sub = subscriptions.values().any(|s| s.client_id == *client_id);

                // Debug log: Subscription control state
                if let Some(ref path) = *self.debug_log_path.read().await {
                    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                        use std::io::Write;
                        let _ = writeln!(
                            f,
                            "[{}] [SubscriptionHub] Client {} subscription state: has_any={}, is_controlling={}",
                            chrono::Local::now().format("%H:%M:%S%.3f"),
                            client_id,
                            has_any_sub,
                            controlling_sub.is_some()
                        );
                        if let Some(sub) = controlling_sub {
                            let _ = writeln!(
                                f,
                                "[{}] [SubscriptionHub] Controlling subscription: id={}",
                                chrono::Local::now().format("%H:%M:%S%.3f"),
                                sub.id
                            );
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
                                let _ = writeln!(
                                    f,
                                    "[{}] [SubscriptionHub] Forwarding {} bytes to PTY writer",
                                    chrono::Local::now().format("%H:%M:%S%.3f"),
                                    data.len()
                                );
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
                                        let _ = writeln!(
                                            f,
                                            "[{}] [SubscriptionHub] PTY write successful, took {}ms",
                                            chrono::Local::now().format("%H:%M:%S%.3f"),
                                            elapsed.as_millis()
                                        );
                                    }
                                    Err(e) => {
                                        let _ = writeln!(
                                            f,
                                            "[{}] [SubscriptionHub] PTY write failed after {}ms: {:?}",
                                            chrono::Local::now().format("%H:%M:%S%.3f"),
                                            elapsed.as_millis(),
                                            e
                                        );
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
                                let _ = writeln!(
                                    f,
                                    "[{}] [SubscriptionHub] WARNING: No PTY writer available",
                                    chrono::Local::now().format("%H:%M:%S%.3f")
                                );
                            }
                        }
                    }

                    // Notify handler
                    if let Some(handler) = self.handler.read().await.as_ref() {
                        handler.on_input(&sub.id, data).await?;
                    }

                    // FIXME: Commenting out force_snapshot workaround that was causing sync issues
                    // Proactively send a fresh snapshot to ensure client render stays in sync
                    // This helps when delta routing/ordering is under investigation
                    // let _ = self.force_snapshot(&sub.id).await;
                } else {
                    // Debug log: Input not allowed
                    if let Some(ref path) = *self.debug_log_path.read().await {
                        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                            use std::io::Write;
                            let _ = writeln!(
                                f,
                                "[{}] [SubscriptionHub] INPUT_NOT_ALLOWED: Client {} does not have controlling subscription",
                                chrono::Local::now().format("%H:%M:%S%.3f"),
                                client_id
                            );
                        }
                    }

                    // Send error to client
                    if let Some(subscription) =
                        subscriptions.values().find(|s| s.client_id == *client_id)
                    {
                        self.send_to_subscription(
                            subscription,
                            ServerMessage::Error {
                                subscription_id: Some(subscription.id.clone()),
                                code: ErrorCode::INPUT_NOT_ALLOWED,
                                message: "Client does not have input permissions".to_string(),
                                recoverable: true,
                                retry_after: None,
                            },
                        )
                        .await?;
                    }
                }
            }

            ClientMessage::ModifySubscription {
                subscription_id,
                dimensions,
                mode,
                position,
            } => {
                // Debug event: ModifySubscriptionReceived
                if let Some(recorder) = self.debug_recorder.read().await.as_ref() {
                    if let Ok(mut rec) = recorder.lock() {
                        let _ = rec.record_event(
                            crate::debug_recorder::DebugEvent::ModifySubscriptionReceived {
                                timestamp: chrono::Utc::now(),
                                subscription_id: subscription_id.clone(),
                                mode: format!("{:?}", mode),
                                position: position.as_ref().map(|p| format!("{:?}", p)),
                            },
                        );
                    }
                }

                // Clone dimensions before moving into update
                let dims_for_handler = dimensions.clone();

                // Clone mode and position for later use
                let mode_clone = mode.clone();
                let position_clone = position.clone();

                let update = SubscriptionUpdate {
                    dimensions,
                    mode,
                    position,
                    ..Default::default()
                };
                self.update(&subscription_id, update).await?;

                // Map legacy ModifySubscription to viewport semantics
                // This allows backward compatibility with clients using the old protocol
                if mode_clone.is_some() || position_clone.is_some() {
                    let mut subscriptions = self.subscriptions.write().await;
                    if let Some(subscription) = subscriptions.get_mut(&subscription_id) {
                        // Convert mode/position to viewport
                        match subscription.mode {
                            ViewMode::Realtime => {
                                // For realtime mode, follow the tail
                                subscription.follow_tail = true;
                                subscription.viewport = None; // No specific viewport, follow tail
                            }
                            ViewMode::Anchored => {
                                // Anchored mode: stay at current position
                                subscription.follow_tail = false;
                                // Keep existing viewport if any, otherwise no change
                            }
                            ViewMode::Historical => {
                                // For historical mode, use position to determine viewport
                                subscription.follow_tail = false;

                                // Calculate viewport based on position
                                if let Some(ref pos) = subscription.position {
                                    let viewport_height = subscription.dimensions.height as u64;
                                    let (start_line, end_line) = if let Some(line) = pos.line {
                                        // Line-based position: center viewport around the requested line
                                        let half_height = viewport_height / 2;
                                        let start = line.saturating_sub(half_height);
                                        let end = start + viewport_height - 1;
                                        (start, end)
                                    } else if pos.time.is_some() {
                                        // Time-based position not yet supported in viewport model
                                        // Default to showing from line 0
                                        (0, viewport_height - 1)
                                    } else {
                                        // No specific position, default to beginning
                                        (0, viewport_height - 1)
                                    };

                                    subscription.viewport =
                                        Some(crate::protocol::subscription::messages::Viewport {
                                            start_line,
                                            end_line,
                                        });

                                    // Set default prefetch for legacy clients
                                    if subscription.prefetch.is_none() {
                                        subscription.prefetch = Some(
                                            crate::protocol::subscription::messages::Prefetch {
                                                before: 24, // Prefetch one screen above
                                                after: 24,  // Prefetch one screen below
                                            },
                                        );
                                    }
                                }
                            }
                        }
                    }
                }

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

                // After viewport mapping, send SnapshotRange if viewport is set
                {
                    let mut subscriptions = self.subscriptions.write().await;
                    if let Some(subscription) = subscriptions.get_mut(&subscription_id) {
                        if let Some(vp) = &subscription.viewport {
                            if let Some(source) = self.terminal_source.read().await.as_ref() {
                                let width = subscription.dimensions.width;
                                let rows =
                                    (vp.end_line - vp.start_line + 1).min(u16::MAX as u64) as u16;

                                if let Ok((grid, watermark)) = source
                                    .snapshot_range_with_watermark(width, vp.start_line, rows)
                                    .await
                                {
                                    subscription.current_sequence =
                                        subscription.current_sequence.saturating_add(1);
                                    let message = ServerMessage::SnapshotRange {
                                        subscription_id: subscription_id.clone(),
                                        sequence: subscription.current_sequence,
                                        watermark_seq: watermark,
                                        grid,
                                        timestamp: chrono::Utc::now().timestamp(),
                                        checksum: 0,
                                    };
                                    self.send_to_subscription(subscription, message).await?;
                                }
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

            ClientMessage::RequestState {
                subscription_id, ..
            } => {
                self.force_snapshot(&subscription_id).await?;
            }

            ClientMessage::ViewportChanged {
                subscription_id,
                viewport,
                prefetch,
                follow_tail,
            } => {
                // Update the viewport for the subscription
                let sub_clone = {
                    let mut subscriptions = self.subscriptions.write().await;
                    if let Some(subscription) = subscriptions.get_mut(&subscription_id) {
                        // Calculate scroll velocity for adaptive prefetching
                        let current_time = chrono::Utc::now();
                        if let (Some(prev_viewport), Some(last_change)) = (
                            &subscription.previous_viewport,
                            subscription.last_viewport_change,
                        ) {
                            let time_diff =
                                current_time.timestamp_millis() - last_change.timestamp_millis();
                            if time_diff > 0 {
                                let line_diff = (viewport.start_line as i64)
                                    - (prev_viewport.start_line as i64);
                                subscription.scroll_velocity =
                                    (line_diff as f64 * 1000.0) / (time_diff as f64); // lines per second
                            }
                        }

                        // Update viewport tracking fields
                        subscription.previous_viewport = subscription.viewport.clone();
                        subscription.last_viewport_change = Some(current_time);
                        subscription.viewport = Some(viewport.clone());
                        subscription.prefetch = prefetch.clone();
                        if let Some(follow) = follow_tail {
                            subscription.follow_tail = follow;
                        }

                        // Clone the subscription for later use
                        Some(Subscription {
                            id: subscription.id.clone(),
                            client_id: subscription.client_id.clone(),
                            dimensions: subscription.dimensions.clone(),
                            mode: subscription.mode.clone(),
                            position: subscription.position.clone(),
                            transport: subscription.transport.clone(),
                            is_controlling: subscription.is_controlling,
                            last_sequence_acked: subscription.last_sequence_acked,
                            current_sequence: subscription.current_sequence + 1,
                            previous_grid: subscription.previous_grid.clone(),
                            viewport: Some(viewport.clone()),
                            prefetch: prefetch.clone(),
                            follow_tail: subscription.follow_tail,
                            watermark_seq: subscription.watermark_seq,
                            previous_viewport: subscription.previous_viewport.clone(),
                            last_viewport_change: subscription.last_viewport_change,
                            scroll_velocity: subscription.scroll_velocity,
                        })
                    } else {
                        None
                    }
                };

                // If we have a subscription and data source, send the new viewport range
                if let Some(sub) = sub_clone {
                    if let Some(source) = self.terminal_source.read().await.as_ref() {
                        // Calculate the visible range with prefetch
                        let prefetch_before =
                            prefetch.as_ref().map(|p| p.before).unwrap_or(0) as u64;
                        let prefetch_after = prefetch.as_ref().map(|p| p.after).unwrap_or(0) as u64;

                        let start_line = viewport.start_line.saturating_sub(prefetch_before);
                        let end_line = viewport.end_line + prefetch_after;
                        let rows = (end_line - start_line + 1).min(u16::MAX as u64) as u16;

                        // Debug: log incoming viewport change and derived request
                        if let Some(ref path) = *self.debug_log_path.read().await {
                            if let Ok(mut f) = std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(path)
                            {
                                use std::io::Write;
                                let _ = writeln!(
                                    f,
                                    "[{}] [SubscriptionHub] ViewportChanged recv sub={} vp=({}, {}) prefetch=({},{}) follow_tail={}",
                                    chrono::Local::now().format("%H:%M:%S%.3f"),
                                    subscription_id,
                                    viewport.start_line,
                                    viewport.end_line,
                                    prefetch_before,
                                    prefetch_after,
                                    sub.follow_tail
                                );
                                let _ = writeln!(
                                    f,
                                    "[{}] [SubscriptionHub] SnapshotRange request: width={} start_line={} rows={}",
                                    chrono::Local::now().format("%H:%M:%S%.3f"),
                                    sub.dimensions.width,
                                    start_line,
                                    rows
                                );
                            }
                        }

                        // Get the snapshot with watermark
                        if let Ok((grid, watermark)) = source
                            .snapshot_range_with_watermark(sub.dimensions.width, start_line, rows)
                            .await
                        {
                            // Debug: log the actual grid content retrieved
                            if let Some(ref path) = *self.debug_log_path.read().await {
                                if let Ok(mut f) = std::fs::OpenOptions::new()
                                    .create(true)
                                    .append(true)
                                    .open(path)
                                {
                                    use std::io::Write;

                                    // Sample the first few rows of the retrieved grid
                                    let mut sample_content = Vec::new();
                                    let mut non_blank_rows = 0;
                                    for row in 0..grid.height.min(5) {
                                        let mut line = String::new();
                                        for col in 0..grid.width.min(80) {
                                            if let Some(cell) = grid.get_cell(row, col) {
                                                line.push(cell.char);
                                            }
                                        }
                                        let trimmed = line.trim_end();
                                        if !trimmed.is_empty() {
                                            non_blank_rows += 1;
                                        }
                                        sample_content.push(format!("row[{}]: '{}'", row, trimmed));
                                    }

                                    let _ = writeln!(
                                        f,
                                        "[{}] [SubscriptionHub] GRID_RETRIEVED for sub={} grid_range=({:?},{:?}) dims={}x{} non_blank_rows={}/5",
                                        chrono::Local::now().format("%H:%M:%S%.3f"),
                                        subscription_id,
                                        grid.start_line.to_u64(),
                                        grid.end_line.to_u64(),
                                        grid.width,
                                        grid.height,
                                        non_blank_rows
                                    );
                                    let _ = writeln!(
                                        f,
                                        "[{}] [SubscriptionHub] GRID_CONTENT: {}",
                                        chrono::Local::now().format("%H:%M:%S%.3f"),
                                        sample_content.join(", ")
                                    );
                                }
                            }
                            // Update watermark in the subscription
                            {
                                let mut subscriptions = self.subscriptions.write().await;
                                if let Some(subscription) = subscriptions.get_mut(&subscription_id)
                                {
                                    subscription.watermark_seq = watermark;
                                    subscription.current_sequence += 1;
                                }
                            }

                            // Send SnapshotRange message
                            let message = ServerMessage::SnapshotRange {
                                subscription_id: subscription_id.clone(),
                                sequence: sub.current_sequence,
                                watermark_seq: watermark,
                                grid: grid.clone(),
                                timestamp: chrono::Utc::now().timestamp(),
                                checksum: 0, // TODO: Calculate checksum
                            };

                            // TODO: Use Control channel when dual-channel is implemented
                            self.send_to_subscription(&sub, message).await?;
                            // Debug: log returned grid range
                            if let Some(ref path) = *self.debug_log_path.read().await {
                                if let Ok(mut f) = std::fs::OpenOptions::new()
                                    .create(true)
                                    .append(true)
                                    .open(path)
                                {
                                    use std::io::Write;
                                    let _ = writeln!(
                                        f,
                                        "[{}] [SubscriptionHub] SnapshotRange result sub={} wm={} grid_range=({:?}, {:?}) dims={}x{} seq={}",
                                        chrono::Local::now().format("%H:%M:%S%.3f"),
                                        subscription_id,
                                        watermark,
                                        grid.start_line.to_u64(),
                                        grid.end_line.to_u64(),
                                        grid.width,
                                        grid.height,
                                        sub.current_sequence
                                    );
                                }
                            }

                            // Trigger proactive background prefetch for smooth scrolling
                            // Only prefetch if not following tail and have significant scroll velocity
                            if !sub.follow_tail && sub.scroll_velocity.abs() > 0.5 {
                                let _ = self.request_background_prefetch(&subscription_id).await;
                            }
                        }
                    }
                }
            }

            _ => {
                // Other messages handled elsewhere
            }
        }

        Ok(())
    }

    /// Send a message to a specific subscription with channel routing
    /// Check which region a line falls into relative to viewport and prefetch
    fn get_line_region(&self, line_num: u64, subscription: &Subscription) -> LineRegion {
        if let Some(viewport) = &subscription.viewport {
            // Check if line is in immediate viewport
            if line_num >= viewport.start_line && line_num <= viewport.end_line {
                return LineRegion::Viewport;
            }

            // Check if line is in prefetch region
            if let Some(prefetch) = &subscription.prefetch {
                let prefetch_start = viewport.start_line.saturating_sub(prefetch.before as u64);
                let prefetch_end = viewport.end_line + prefetch.after as u64;

                if line_num >= prefetch_start && line_num <= prefetch_end {
                    return LineRegion::Prefetch;
                }
            }
        }

        LineRegion::Background
    }

    /// Determine priority for a message based on viewport and prefetch status
    fn determine_message_priority(
        &self,
        subscription: &Subscription,
        message: &ServerMessage,
    ) -> MessagePriority {
        match message {
            ServerMessage::SnapshotRange { grid, .. } => {
                // For SnapshotRange, determine priority based on which region they cover
                {
                    // Check the region covered by this snapshot
                    let snapshot_start = grid.start_line.to_u64().unwrap_or(0);
                    let snapshot_end = grid.end_line.to_u64().unwrap_or(0);

                    // Determine the most important region this snapshot covers
                    let mut has_viewport = false;
                    let mut has_prefetch = false;

                    for line in snapshot_start..=snapshot_end {
                        match self.get_line_region(line, subscription) {
                            LineRegion::Viewport => {
                                has_viewport = true;
                                break;
                            }
                            LineRegion::Prefetch => has_prefetch = true,
                            LineRegion::Background => {}
                        }
                    }

                    if has_viewport {
                        MessagePriority::ViewportSnapshot
                    } else if has_prefetch {
                        MessagePriority::PrefetchSnapshot
                    } else {
                        MessagePriority::BackgroundSnapshot
                    }
                }
            }
            ServerMessage::Snapshot { .. } => {
                // Initial snapshots (when no viewport is set) should always get highest priority
                // to ensure client gets initial screen state immediately
                MessagePriority::ViewportSnapshot
            }
            ServerMessage::Delta { changes, .. } => {
                // If following tail (realtime mode), assume all deltas are viewport priority
                if subscription.follow_tail {
                    return MessagePriority::ViewportDelta;
                }

                // Check if delta affects any lines and determine highest priority region
                let mut highest_priority = MessagePriority::BackgroundDelta;

                for change in &changes.cell_changes {
                    // For viewport-based deltas, the row is absolute within the current grid
                    // We need to map it to actual line numbers based on the grid's line tracking
                    let line_num = change.row as u64; // This is row within current snapshot

                    match self.get_line_region(line_num, subscription) {
                        LineRegion::Viewport => {
                            return MessagePriority::ViewportDelta; // Highest possible, return immediately
                        }
                        LineRegion::Prefetch => {
                            // Update but continue checking in case we find viewport
                            if highest_priority < MessagePriority::PrefetchSnapshot {
                                highest_priority = MessagePriority::PrefetchSnapshot;
                            }
                        }
                        LineRegion::Background => {
                            // Keep current priority
                        }
                    }
                }

                highest_priority
            }
            // Control messages (acks, errors, etc.) use viewport priority to ensure delivery
            _ => MessagePriority::ViewportSnapshot,
        }
    }

    /// Queue a message for sending with priority
    async fn enqueue_message(
        &self,
        subscription: &Subscription,
        message: ServerMessage,
    ) -> Result<()> {
        let priority = self.determine_message_priority(subscription, &message);
        // Debug: log enqueue intent with message kind and priority
        if let Some(ref path) = *self.debug_log_path.read().await {
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                use std::io::Write;
                let msg_kind = match &message {
                    ServerMessage::Snapshot { .. } => "Snapshot",
                    ServerMessage::SnapshotRange { .. } => "SnapshotRange",
                    ServerMessage::Delta { .. } => "Delta",
                    ServerMessage::DeltaBatch { .. } => "DeltaBatch",
                    ServerMessage::ViewTransition { .. } => "ViewTransition",
                    ServerMessage::SubscriptionAck { .. } => "SubscriptionAck",
                    ServerMessage::Error { .. } => "Error",
                    ServerMessage::Pong { .. } => "Pong",
                    ServerMessage::Notify { .. } => "Notify",
                    ServerMessage::HistoryInfo { .. } => "HistoryInfo",
                    ServerMessage::HistoryMetadata { .. } => "HistoryMetadata",
                    ServerMessage::HistoryChunk { .. } => "HistoryChunk",
                };
                let _ = writeln!(
                    f,
                    "[{}] [SubscriptionHub] enqueue_message: sub={} kind={} priority={:?}",
                    chrono::Local::now().format("%H:%M:%S%.3f"),
                    subscription.id,
                    msg_kind,
                    priority
                );
            }
        }
        let prioritized_msg = PrioritizedMessage {
            priority,
            subscription_id: subscription.id.clone(),
            message,
            timestamp: chrono::Utc::now(),
        };

        let mut queue = self.send_queue.write().await;
        queue.push(prioritized_msg);
        // Debug: log queue length after push
        if let Some(ref path) = *self.debug_log_path.read().await {
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                use std::io::Write;
                let _ = writeln!(
                    f,
                    "[{}] [SubscriptionHub] enqueue_message: queue_len={} (after push)",
                    chrono::Local::now().format("%H:%M:%S%.3f"),
                    queue.len()
                );
            }
        }

        // Always trigger background processing via wake signal (avoid inline processing while holding locks)
        drop(queue);
        if let Some(queue_tx) = self.queue_tx.read().await.as_ref() {
            let _ = queue_tx.try_send(());
            // Debug: log background wake
            if let Some(ref path) = *self.debug_log_path.read().await {
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                {
                    use std::io::Write;
                    let _ = writeln!(
                        f,
                        "[{}] [SubscriptionHub] enqueue_message: wake sent for background processing",
                        chrono::Local::now().format("%H:%M:%S%.3f")
                    );
                }
            }
        }

        Ok(())
    }

    /// Process messages from the priority queue
    async fn process_priority_queue(&self) -> Result<()> {
        // Debug: log entry and current queue length
        if let Some(ref path) = *self.debug_log_path.read().await {
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                use std::io::Write;
                let qlen = { self.send_queue.read().await.len() };
                let _ = writeln!(
                    f,
                    "[{}] [SubscriptionHub] process_priority_queue: start queue_len={}",
                    chrono::Local::now().format("%H:%M:%S%.3f"),
                    qlen
                );
            }
        }
        // Process up to 10 messages at a time to avoid blocking
        for _ in 0..10 {
            let prioritized_msg = {
                let mut queue = self.send_queue.write().await;
                queue.pop()
            };

            if let Some(msg) = prioritized_msg {
                // Debug: log popped message info
                if let Some(ref path) = *self.debug_log_path.read().await {
                    if let Ok(mut f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                    {
                        use std::io::Write;
                        let kind = match &msg.message {
                            ServerMessage::Snapshot { .. } => "Snapshot",
                            ServerMessage::SnapshotRange { .. } => "SnapshotRange",
                            ServerMessage::Delta { .. } => "Delta",
                            ServerMessage::DeltaBatch { .. } => "DeltaBatch",
                            ServerMessage::ViewTransition { .. } => "ViewTransition",
                            ServerMessage::SubscriptionAck { .. } => "SubscriptionAck",
                            ServerMessage::Error { .. } => "Error",
                            ServerMessage::Pong { .. } => "Pong",
                            ServerMessage::Notify { .. } => "Notify",
                            ServerMessage::HistoryInfo { .. } => "HistoryInfo",
                            ServerMessage::HistoryMetadata { .. } => "HistoryMetadata",
                            ServerMessage::HistoryChunk { .. } => "HistoryChunk",
                        };
                        let _ = writeln!(
                            f,
                            "[{}] [SubscriptionHub] process_priority_queue: pop sub={} kind={} priority={:?}",
                            chrono::Local::now().format("%H:%M:%S%.3f"),
                            msg.subscription_id,
                            kind,
                            msg.priority
                        );
                    }
                }
                let subscription = {
                    let subscriptions = self.subscriptions.read().await;
                    subscriptions.get(&msg.subscription_id).cloned()
                };

                if let Some(sub) = subscription {
                    // Debug: log about to send
                    if let Some(ref path) = *self.debug_log_path.read().await {
                        if let Ok(mut f) = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(path)
                        {
                            use std::io::Write;
                            let _ = writeln!(
                                f,
                                "[{}] [SubscriptionHub] process_priority_queue: delivering to sub={}",
                                chrono::Local::now().format("%H:%M:%S%.3f"),
                                sub.id
                            );
                        }
                    }
                    let _ = self.send_message_direct(&sub, msg.message).await;
                } else {
                    // Subscription no longer exists, skip message
                    if let Some(ref path) = *self.debug_log_path.read().await {
                        if let Ok(mut f) = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(path)
                        {
                            use std::io::Write;
                            let _ = writeln!(
                                f,
                                "[{}] [SubscriptionHub] process_priority_queue: missing sub={}, dropping message",
                                chrono::Local::now().format("%H:%M:%S%.3f"),
                                msg.subscription_id
                            );
                        }
                    }
                }
            } else {
                // Debug: queue empty
                if let Some(ref path) = *self.debug_log_path.read().await {
                    if let Ok(mut f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                    {
                        use std::io::Write;
                        let _ = writeln!(
                            f,
                            "[{}] [SubscriptionHub] process_priority_queue: queue empty",
                            chrono::Local::now().format("%H:%M:%S%.3f")
                        );
                    }
                }
                break;
            }
        }

        Ok(())
    }

    async fn send_to_subscription(
        &self,
        subscription: &Subscription,
        message: ServerMessage,
    ) -> Result<()> {
        // Use prioritized sending for delta and snapshot messages
        match &message {
            ServerMessage::Delta { .. }
            | ServerMessage::Snapshot { .. }
            | ServerMessage::SnapshotRange { .. } => {
                // Debug: note prioritized path
                if let Some(ref path) = *self.debug_log_path.read().await {
                    if let Ok(mut f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                    {
                        use std::io::Write;
                        let kind = match &message {
                            ServerMessage::Snapshot { .. } => "Snapshot",
                            ServerMessage::SnapshotRange { .. } => "SnapshotRange",
                            ServerMessage::Delta { .. } => "Delta",
                            _ => "Other",
                        };
                        let _ = writeln!(
                            f,
                            "[{}] [SubscriptionHub] send_to_subscription(prioritized): sub={} kind={}",
                            chrono::Local::now().format("%H:%M:%S%.3f"),
                            subscription.id,
                            kind
                        );
                    }
                }
                self.enqueue_message(subscription, message).await
            }
            // Send control messages directly for immediate delivery
            _ => {
                // Debug: note direct path
                if let Some(ref path) = *self.debug_log_path.read().await {
                    if let Ok(mut f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                    {
                        use std::io::Write;
                        let kind = match &message {
                            ServerMessage::SubscriptionAck { .. } => "SubscriptionAck",
                            ServerMessage::HistoryInfo { .. } => "HistoryInfo",
                            ServerMessage::Error { .. } => "Error",
                            _ => "Other",
                        };
                        let _ = writeln!(
                            f,
                            "[{}] [SubscriptionHub] send_to_subscription(direct): sub={} kind={}",
                            chrono::Local::now().format("%H:%M:%S%.3f"),
                            subscription.id,
                            kind
                        );
                    }
                }
                self.send_message_direct(subscription, message).await
            }
        }
    }

    async fn send_message_direct(
        &self,
        subscription: &Subscription,
        message: ServerMessage,
    ) -> Result<()> {
        // Log what we're about to send
        if let Some(ref path) = *self.debug_log_path.read().await {
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                use std::io::Write;
                match &message {
                    ServerMessage::Snapshot {
                        subscription_id, ..
                    } => {
                        let _ = writeln!(
                            f,
                            "[{}] [SubscriptionHub] send_to_subscription: Sending Snapshot to {}",
                            chrono::Local::now().format("%H:%M:%S%.3f"),
                            subscription_id
                        );
                    }
                    ServerMessage::Delta {
                        subscription_id,
                        sequence,
                        ..
                    } => {
                        let _ = writeln!(
                            f,
                            "[{}] [SubscriptionHub] send_to_subscription: Sending Delta seq {} to {}",
                            chrono::Local::now().format("%H:%M:%S%.3f"),
                            sequence,
                            subscription_id
                        );
                    }
                    ServerMessage::SubscriptionAck {
                        subscription_id, ..
                    } => {
                        let _ = writeln!(
                            f,
                            "[{}] [SubscriptionHub] send_to_subscription: Sending SubscriptionAck to {}",
                            chrono::Local::now().format("%H:%M:%S%.3f"),
                            subscription_id
                        );
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
        // Debug: message type + size
        if let Some(ref path) = *self.debug_log_path.read().await {
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                use std::io::Write;
                let msg_kind = match &message {
                    ServerMessage::Snapshot { .. } => "Snapshot",
                    ServerMessage::SnapshotRange { .. } => "SnapshotRange",
                    ServerMessage::Delta { .. } => "Delta",
                    ServerMessage::DeltaBatch { .. } => "DeltaBatch",
                    ServerMessage::ViewTransition { .. } => "ViewTransition",
                    ServerMessage::SubscriptionAck { .. } => "SubscriptionAck",
                    ServerMessage::Error { .. } => "Error",
                    ServerMessage::Pong { .. } => "Pong",
                    ServerMessage::Notify { .. } => "Notify",
                    ServerMessage::HistoryInfo { .. } => "HistoryInfo",
                    ServerMessage::HistoryMetadata { .. } => "HistoryMetadata",
                    ServerMessage::HistoryChunk { .. } => "HistoryChunk",
                };
                let _ = writeln!(
                    f,
                    "[{}] [SubscriptionHub] send_to_subscription: {} bytes={} sub={}",
                    chrono::Local::now().format("%H:%M:%S%.3f"),
                    msg_kind,
                    bytes.len(),
                    subscription.id
                );
            }
        }

        // TEMP: Send via legacy/default transport to ensure client receive loop gets frames
        let send_result = subscription.transport.send(&bytes).await;

        // Log send result
        if let Some(ref path) = *self.debug_log_path.read().await {
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
                use std::io::Write;
                match &send_result {
                    Ok(_) => {
                        let _ = writeln!(
                            f,
                            "[{}] [SubscriptionHub] send_to_subscription: Successfully sent message",
                            chrono::Local::now().format("%H:%M:%S%.3f")
                        );
                    }
                    Err(e) => {
                        let _ = writeln!(
                            f,
                            "[{}] [SubscriptionHub] send_to_subscription: Failed to send: {:?}",
                            chrono::Local::now().format("%H:%M:%S%.3f"),
                            e
                        );
                    }
                }
            }
        }

        send_result?;
        Ok(())
    }

    /// Request background prefetch snapshots for smooth scrolling
    /// This proactively fetches data beyond the prefetch region to prepare for potential scrolling
    pub async fn request_background_prefetch(&self, subscription_id: &str) -> Result<()> {
        let subscription = {
            let subscriptions = self.subscriptions.read().await;
            subscriptions.get(subscription_id).cloned()
        };

        if let Some(sub) = subscription {
            if let (Some(viewport), Some(prefetch)) = (&sub.viewport, &sub.prefetch) {
                if let Some(source) = self.terminal_source.read().await.as_ref() {
                    // Calculate adaptive prefetch regions based on scroll velocity
                    let velocity_multiplier = (sub.scroll_velocity.abs() / 10.0).max(1.0).min(5.0); // Scale 1x-5x based on velocity
                    let extended_prefetch_before =
                        ((prefetch.before as f64) * velocity_multiplier) as u64;
                    let extended_prefetch_after =
                        ((prefetch.after as f64) * velocity_multiplier) as u64;

                    // Request snapshots for extended regions beyond current prefetch
                    let background_start =
                        viewport.start_line.saturating_sub(extended_prefetch_before);
                    let background_end = viewport.end_line + extended_prefetch_after;

                    // Only fetch areas outside the current prefetch region
                    let current_prefetch_start =
                        viewport.start_line.saturating_sub(prefetch.before as u64);
                    let current_prefetch_end = viewport.end_line + prefetch.after as u64;

                    // Background region before current prefetch
                    if background_start < current_prefetch_start {
                        let rows =
                            (current_prefetch_start - background_start).min(u16::MAX as u64) as u16;
                        if let Ok((grid, watermark)) = source
                            .snapshot_range_with_watermark(
                                sub.dimensions.width,
                                background_start,
                                rows,
                            )
                            .await
                        {
                            let message = ServerMessage::SnapshotRange {
                                subscription_id: subscription_id.to_string(),
                                sequence: sub.current_sequence + 1,
                                watermark_seq: watermark,
                                grid,
                                timestamp: chrono::Utc::now().timestamp(),
                                checksum: 0,
                            };

                            // Use background priority for this request
                            self.enqueue_message(&sub, message).await?;
                        }
                    }

                    // Background region after current prefetch
                    if background_end > current_prefetch_end {
                        let start = current_prefetch_end + 1;
                        let rows = (background_end - start + 1).min(u16::MAX as u64) as u16;
                        if let Ok((grid, watermark)) = source
                            .snapshot_range_with_watermark(sub.dimensions.width, start, rows)
                            .await
                        {
                            let message = ServerMessage::SnapshotRange {
                                subscription_id: subscription_id.to_string(),
                                sequence: sub.current_sequence + 2,
                                watermark_seq: watermark,
                                grid,
                                timestamp: chrono::Utc::now().timestamp(),
                                checksum: 0,
                            };

                            // Use background priority for this request
                            self.enqueue_message(&sub, message).await?;
                        }
                    }
                }
            }
        }

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

    /// Handle unified grid subscription setup (NEW)
    async fn handle_unified_grid_subscription(
        &self,
        subscription: &Subscription,
        subscription_id: &str,
        source: &Arc<dyn TerminalDataSource>,
        config: &SubscriptionConfig,
    ) -> Result<()> {
        // Get terminal history metadata
        let metadata = source.get_history_metadata().await?;

        // Send HistoryMetadata first
        self.send_to_subscription(
            subscription,
            ServerMessage::HistoryMetadata {
                subscription_id: subscription_id.to_string(),
                total_lines: metadata.total_lines,
                oldest_line: metadata.oldest_line,
                latest_line: metadata.latest_line,
                terminal_width: config.dimensions.width,
                terminal_height: config.dimensions.height,
            },
        )
        .await?;

        // Calculate initial fetch size (smaller to avoid WebRTC limits)
        let initial_fetch = std::cmp::min(config.initial_fetch_size.unwrap_or(50), 50);

        // Send initial snapshot with most recent rows
        let start_line = metadata
            .latest_line
            .saturating_sub(initial_fetch as u64 - 1);
        let (initial_grid, _) = source
            .snapshot_range_with_watermark(
                config.dimensions.width,
                start_line,
                initial_fetch as u16,
            )
            .await?;

        // Send initial snapshot directly (not queued) to ensure it arrives before HistoryChunk messages
        self.send_message_direct(
            subscription,
            ServerMessage::Snapshot {
                subscription_id: subscription_id.to_string(),
                sequence: 0,
                grid: initial_grid,
                timestamp: chrono::Utc::now().timestamp(),
                checksum: 0,
            },
        )
        .await?;

        // Brief delay to ensure initial snapshot is transmitted before history chunks
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Send remaining history if there's any before the initial snapshot
        if start_line > metadata.oldest_line {
            let remaining_lines = start_line - metadata.oldest_line;
            if remaining_lines > 0 {
                // Send history in smaller chunks to avoid WebRTC message size limits
                const CHUNK_SIZE: u64 = 5; // Send 5 rows at a time to keep messages small
                let mut current_line = metadata.oldest_line;

                while current_line < start_line {
                    let chunk_end = std::cmp::min(current_line + CHUNK_SIZE - 1, start_line - 1);
                    let is_final_chunk = chunk_end >= start_line - 1;

                    let chunk_rows = (chunk_end - current_line + 1) as u16;
                    if let Ok((chunk_grid, _)) = source
                        .snapshot_range_with_watermark(
                            config.dimensions.width,
                            current_line,
                            chunk_rows,
                        )
                        .await
                    {
                        let rows = chunk_grid.cells;
                        self.send_to_subscription(
                            subscription,
                            ServerMessage::HistoryChunk {
                                subscription_id: subscription_id.to_string(),
                                start_line: current_line,
                                end_line: chunk_end,
                                rows,
                                is_final: is_final_chunk,
                            },
                        )
                        .await?;

                        if let Some(ref debug_log_path) = *self.debug_log_path.read().await {
                            if let Ok(mut f) = std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(debug_log_path)
                            {
                                use std::io::Write;
                                let _ = writeln!(
                                    f,
                                    "[{}] [SubscriptionHub] Sent history chunk for {} (lines {}-{}, final={})",
                                    chrono::Local::now().format("%H:%M:%S%.3f"),
                                    subscription_id,
                                    current_line,
                                    chunk_end,
                                    is_final_chunk
                                );
                            }
                        }
                    }

                    current_line = chunk_end + 1;
                }
            }
        } else {
            if let Some(ref debug_log_path) = *self.debug_log_path.read().await {
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(debug_log_path)
                {
                    use std::io::Write;
                    let _ = writeln!(
                        f,
                        "[{}] [SubscriptionHub] No additional history to stream for {}",
                        chrono::Local::now().format("%H:%M:%S%.3f"),
                        subscription_id
                    );
                }
            }
        }

        Ok(())
    }

    /// Stream remaining history to client in chunks (NEW)
    async fn stream_history_to_client(
        &self,
        subscription_id: String,
        oldest_line: u64,
        already_sent_end: u64,
        latest_line: u64,
    ) -> Result<()> {
        let chunk_size = 200u64; // Rows per chunk
        let delay_between_chunks = std::time::Duration::from_millis(50); // Rate limiting

        // Stream older history (before the initial snapshot)
        if already_sent_end > oldest_line {
            let mut current_line = already_sent_end.saturating_sub(chunk_size);

            while current_line > oldest_line {
                let end_line = (current_line + chunk_size - 1).min(already_sent_end - 1);

                if let Err(_) = self
                    .send_history_chunk(&subscription_id, current_line, end_line, false)
                    .await
                {
                    return Ok(()); // Client disconnected
                }

                current_line = current_line.saturating_sub(chunk_size);
                tokio::time::sleep(delay_between_chunks).await;
            }

            // Send the final chunk from oldest_line to current_line
            if oldest_line < already_sent_end {
                let _ = self
                    .send_history_chunk(
                        &subscription_id,
                        oldest_line,
                        current_line.min(already_sent_end - 1),
                        true,
                    )
                    .await;
            }
        }

        Ok(())
    }

    /// Send a single history chunk to a subscription (NEW)
    async fn send_history_chunk(
        &self,
        subscription_id: &str,
        start_line: u64,
        end_line: u64,
        is_final: bool,
    ) -> Result<()> {
        // Get the subscription to send to
        let subscription = {
            let subscriptions = self.subscriptions.read().await;
            subscriptions.get(subscription_id).cloned()
        };

        let Some(sub) = subscription else {
            return Ok(()); // Subscription no longer exists
        };

        // Get the data source
        let source = self.terminal_source.read().await;
        let Some(source) = source.as_ref() else {
            return Ok(());
        };

        // Get history data for this range
        if let Ok(rows) = self
            .get_history_range(source, start_line, end_line, sub.dimensions.width)
            .await
        {
            self.send_to_subscription(
                &sub,
                ServerMessage::HistoryChunk {
                    subscription_id: subscription_id.to_string(),
                    start_line,
                    end_line,
                    rows,
                    is_final,
                },
            )
            .await?;
        }

        Ok(())
    }

    /// Get history rows for a specific range (NEW)
    async fn get_history_range(
        &self,
        source: &Arc<dyn TerminalDataSource>,
        start_line: u64,
        end_line: u64,
        width: u16,
    ) -> Result<Vec<Vec<crate::server::terminal_state::Cell>>> {
        let count = (end_line - start_line + 1) as u16;
        let (grid, _) = source
            .snapshot_range_with_watermark(width, start_line, count)
            .await?;
        Ok(grid.cells)
    }
}

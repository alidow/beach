use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, RwLock};
use anyhow::{Result, anyhow};

use crate::protocol::{
    ClientMessage, ServerMessage, ViewMode, ViewPosition, Dimensions,
    ErrorCode, SubscriptionStatus, NotificationType, SubscriptionInfo
};
use crate::server::terminal_state::{Grid, GridDelta, GridView, TerminalStateTracker};
use crate::transport::Transport;

use super::view_registry::{ViewRegistry, ViewKey, ViewId, ViewInfo, SubscriptionId, ClientId};
use super::subscription_pool::{SubscriptionPool, Subscription, PoolStatus};

pub type ClientConnection = mpsc::Sender<ServerMessage>;

#[derive(Debug)]
pub struct ClientContext {
    pub id: ClientId,
    pub connection: ClientConnection,
    pub is_controlling: bool,
}

pub struct SessionBroker<T: Transport + Send + 'static> {
    view_registry: Arc<RwLock<ViewRegistry>>,
    subscription_pool: Arc<RwLock<SubscriptionPool>>,
    clients: Arc<RwLock<HashMap<ClientId, ClientContext>>>,
    terminal_tracker: Arc<Mutex<TerminalStateTracker>>,
    grid_views: Arc<RwLock<HashMap<ViewId, Arc<Mutex<GridView>>>>>,
    sequence_counter: Arc<RwLock<u64>>,
    transport: T,
}

impl<T: Transport + Send + 'static> SessionBroker<T> {
    pub fn new(transport: T, terminal_tracker: Arc<Mutex<TerminalStateTracker>>) -> Self {
        Self {
            view_registry: Arc::new(RwLock::new(ViewRegistry::new())),
            subscription_pool: Arc::new(RwLock::new(SubscriptionPool::new())),
            clients: Arc::new(RwLock::new(HashMap::new())),
            terminal_tracker,
            grid_views: Arc::new(RwLock::new(HashMap::new())),
            sequence_counter: Arc::new(RwLock::new(0)),
            transport,
        }
    }
    
    pub async fn add_client(&self, client_id: ClientId, connection: ClientConnection, is_controlling: bool) {
        let mut clients = self.clients.write().await;
        clients.insert(client_id.clone(), ClientContext {
            id: client_id,
            connection,
            is_controlling,
        });
    }
    
    pub async fn remove_client(&self, client_id: &str) -> Result<()> {
        let mut clients = self.clients.write().await;
        clients.remove(client_id);
        
        let mut pool = self.subscription_pool.write().await;
        let removed_subscriptions = pool.remove_client(client_id);
        
        let mut registry = self.view_registry.write().await;
        for subscription in removed_subscriptions {
            if let Some(view) = registry.get_view_mut(&subscription.view_id) {
                view.remove_subscriber(&subscription.id);
                if view.is_empty() {
                    self.unregister_view(&subscription.view_id).await?;
                }
            }
        }
        
        Ok(())
    }
    
    pub async fn handle_client_message(&self, client_id: ClientId, message: ClientMessage) -> Result<()> {
        match message {
            ClientMessage::Subscribe { subscription_id, dimensions, mode, position, compression } => {
                self.handle_subscribe(client_id, subscription_id, dimensions, mode, position, compression).await?;
            }
            ClientMessage::ModifySubscription { subscription_id, dimensions, mode, position } => {
                self.handle_modify_subscription(client_id, subscription_id, dimensions, mode, position).await?;
            }
            ClientMessage::Unsubscribe { subscription_id } => {
                self.handle_unsubscribe(client_id, subscription_id).await?;
            }
            ClientMessage::TerminalInput { data, .. } => {
                self.handle_terminal_input(client_id, data).await?;
            }
            ClientMessage::RequestState { subscription_id, request_type, sequence } => {
                self.handle_request_state(client_id, subscription_id, request_type.into(), sequence).await?;
            }
            ClientMessage::Acknowledge { message_id, checksum } => {
                self.handle_acknowledge(client_id, message_id, checksum).await?;
            }
            ClientMessage::Control { control_type, subscription_id } => {
                self.handle_control(client_id, control_type.into(), subscription_id).await?;
            }
            ClientMessage::Ping { timestamp, subscriptions } => {
                self.handle_ping(client_id, timestamp, subscriptions).await?;
            }
        }
        Ok(())
    }
    
    async fn handle_subscribe(
        &self,
        client_id: ClientId,
        subscription_id: SubscriptionId,
        dimensions: Dimensions,
        mode: ViewMode,
        position: Option<ViewPosition>,
        compression: Option<crate::protocol::CompressionType>,
    ) -> Result<()> {
        let view_key = ViewKey::new(dimensions, mode, position);
        
        let mut registry = self.view_registry.write().await;
        let (view_id, is_new) = registry.find_or_create_view(view_key.clone());
        
        if is_new {
            self.register_view(view_id.clone(), view_key.clone()).await?;
        }
        
        if let Some(view) = registry.get_view_mut(&view_id) {
            view.add_subscriber(subscription_id.clone());
        }
        
        let mut pool = self.subscription_pool.write().await;
        let subscription = Subscription::new(
            subscription_id.clone(),
            client_id.clone(),
            view_id.clone(),
            view_key,
            compression,
        );
        let pool_status = pool.add_subscription(subscription);
        
        let (status, shared_with) = match pool_status {
            PoolStatus::Created => (SubscriptionStatus::Active, None),
            PoolStatus::Joined(count) => {
                let other_ids: Vec<String> = pool.get_view_subscriber_ids(&view_id)
                    .into_iter()
                    .filter(|id| id != &subscription_id)
                    .collect();
                (SubscriptionStatus::Shared, Some(other_ids))
            }
        };
        
        self.send_to_client(&client_id, ServerMessage::SubscriptionAck {
            subscription_id: subscription_id.clone(),
            status,
            shared_with,
        }).await?;
        
        let snapshot = self.create_snapshot(&view_id).await?;
        self.send_to_client(&client_id, ServerMessage::Snapshot {
            subscription_id,
            sequence: 0,
            grid: snapshot,
            timestamp: chrono::Utc::now().timestamp(),
            checksum: 0,
        }).await?;
        
        Ok(())
    }
    
    async fn handle_modify_subscription(
        &self,
        client_id: ClientId,
        subscription_id: SubscriptionId,
        dimensions: Option<Dimensions>,
        mode: Option<ViewMode>,
        position: Option<ViewPosition>,
    ) -> Result<()> {
        let mut pool = self.subscription_pool.write().await;
        let subscription = pool.get_subscription(&subscription_id)
            .ok_or_else(|| anyhow!("Unknown subscription: {}", subscription_id))?;
        
        let mut new_key = subscription.view_key.clone();
        if let Some(dims) = dimensions {
            new_key.dimensions = dims;
        }
        if let Some(m) = mode {
            new_key.mode = m;
        }
        if position.is_some() {
            new_key.position = position;
        }
        
        if new_key == subscription.view_key {
            return Ok(());
        }
        
        let old_view_id = subscription.view_id.clone();
        let old_mode = subscription.view_key.mode.clone();
        
        let mut registry = self.view_registry.write().await;
        let (new_view_id, is_new) = registry.find_or_create_view(new_key.clone());
        
        if is_new {
            drop(registry);
            self.register_view(new_view_id.clone(), new_key.clone()).await?;
            registry = self.view_registry.write().await;
        }
        
        if let Some(old_view) = registry.get_view_mut(&old_view_id) {
            old_view.remove_subscriber(&subscription_id);
            if old_view.is_empty() {
                drop(registry);
                self.unregister_view(&old_view_id).await?;
                registry = self.view_registry.write().await;
            }
        }
        
        if let Some(new_view) = registry.get_view_mut(&new_view_id) {
            new_view.add_subscriber(subscription_id.clone());
        }
        
        pool.move_subscription_to_view(&subscription_id, new_view_id.clone(), new_key.clone());
        
        let transition = self.compute_transition(&old_view_id, &new_view_id).await?;
        self.send_to_client(&client_id, ServerMessage::ViewTransition {
            subscription_id,
            from_mode: old_mode,
            to_mode: new_key.mode,
            delta: transition.0,
            snapshot: transition.1,
        }).await?;
        
        Ok(())
    }
    
    async fn handle_unsubscribe(&self, client_id: ClientId, subscription_id: SubscriptionId) -> Result<()> {
        let mut pool = self.subscription_pool.write().await;
        if let Some(subscription) = pool.remove_subscription(&subscription_id) {
            let mut registry = self.view_registry.write().await;
            if let Some(view) = registry.get_view_mut(&subscription.view_id) {
                view.remove_subscriber(&subscription_id);
                if view.is_empty() {
                    self.unregister_view(&subscription.view_id).await?;
                }
            }
        }
        Ok(())
    }
    
    async fn handle_terminal_input(&self, client_id: ClientId, data: Vec<u8>) -> Result<()> {
        let clients = self.clients.read().await;
        if let Some(client) = clients.get(&client_id) {
            if !client.is_controlling {
                self.send_error(&client_id, Some("".to_string()), 
                    ErrorCode::INPUT_NOT_ALLOWED, 
                    "Client does not have input permissions".to_string(),
                    true, None).await?;
                return Ok(());
            }
        }
        
        self.transport.send(&data).await?;
        Ok(())
    }
    
    async fn handle_request_state(
        &self,
        client_id: ClientId,
        subscription_id: SubscriptionId,
        request_type: crate::protocol::StateRequestType,
        sequence: Option<u64>,
    ) -> Result<()> {
        let pool = self.subscription_pool.read().await;
        let subscription = pool.get_subscription(&subscription_id)
            .ok_or_else(|| anyhow!("Unknown subscription: {}", subscription_id))?;
        
        match request_type {
            crate::protocol::StateRequestType::Snapshot => {
                let snapshot = self.create_snapshot(&subscription.view_id).await?;
                self.send_to_client(&client_id, ServerMessage::Snapshot {
                    subscription_id,
                    sequence: sequence.unwrap_or(0),
                    grid: snapshot,
                    timestamp: chrono::Utc::now().timestamp(),
                    checksum: 0,
                }).await?;
            }
            crate::protocol::StateRequestType::Checkpoint => {
                
            }
        }
        
        Ok(())
    }
    
    async fn handle_acknowledge(&self, client_id: ClientId, message_id: String, checksum: Option<u32>) -> Result<()> {
        
        Ok(())
    }
    
    async fn handle_control(
        &self,
        client_id: ClientId,
        control_type: crate::protocol::ControlType,
        subscription_id: Option<String>,
    ) -> Result<()> {
        
        Ok(())
    }
    
    async fn handle_ping(&self, client_id: ClientId, timestamp: i64, subscriptions: Vec<String>) -> Result<()> {
        let pool = self.subscription_pool.read().await;
        let mut subscription_info = HashMap::new();
        
        for sub_id in subscriptions {
            if let Some(subscription) = pool.get_subscription(&sub_id) {
                subscription_info.insert(sub_id, SubscriptionInfo {
                    sequence: subscription.last_sequence_acked,
                    mode: subscription.view_key.mode.clone(),
                });
            }
        }
        
        let sequence = *self.sequence_counter.read().await;
        
        self.send_to_client(&client_id, ServerMessage::Pong {
            timestamp,
            server_sequence: sequence,
            subscriptions: subscription_info,
        }).await?;
        
        Ok(())
    }
    
    async fn register_view(&self, view_id: ViewId, view_key: ViewKey) -> Result<()> {
        let history = self.terminal_tracker.lock().unwrap().get_history();
        let grid_view = Arc::new(Mutex::new(GridView::new(history)));
        
        let mut grid_views = self.grid_views.write().await;
        grid_views.insert(view_id, grid_view);
        
        Ok(())
    }
    
    async fn unregister_view(&self, view_id: &str) -> Result<()> {
        let mut grid_views = self.grid_views.write().await;
        grid_views.remove(view_id);
        
        let mut registry = self.view_registry.write().await;
        registry.remove_view(view_id);
        
        Ok(())
    }
    
    async fn create_snapshot(&self, view_id: &str) -> Result<Grid> {
        let grid_views = self.grid_views.read().await;
        let grid_view = grid_views.get(view_id)
            .ok_or_else(|| anyhow!("View not found: {}", view_id))?;
        
        let registry = self.view_registry.read().await;
        let view_info = registry.get_view(view_id)
            .ok_or_else(|| anyhow!("View info not found: {}", view_id))?;
        
        let dimensions = (view_info.view_key.dimensions.width, view_info.view_key.dimensions.height);
        
        let grid = grid_view.lock().unwrap().derive_realtime(Some(dimensions))?;
        Ok(grid)
    }
    
    async fn compute_transition(&self, from_view_id: &str, to_view_id: &str) -> Result<(Option<GridDelta>, Option<Grid>)> {
        let from_snapshot = self.create_snapshot(from_view_id).await?;
        let to_snapshot = self.create_snapshot(to_view_id).await?;
        
        let delta = GridDelta::diff(&from_snapshot, &to_snapshot);
        
        if delta.cell_changes.len() > (from_snapshot.width as usize * from_snapshot.height as usize / 2) {
            Ok((None, Some(to_snapshot)))
        } else {
            Ok((Some(delta), None))
        }
    }
    
    pub async fn broadcast_delta(&self, view_id: &str, delta: GridDelta) -> Result<()> {
        let pool = self.subscription_pool.read().await;
        let subscribers = pool.get_view_subscribers(view_id);
        
        let mut sequence = self.sequence_counter.write().await;
        *sequence += 1;
        let current_sequence = *sequence;
        
        let message = ServerMessage::Delta {
            subscription_id: "*".to_string(),
            sequence: current_sequence,
            changes: delta,
            timestamp: chrono::Utc::now().timestamp(),
        };
        
        for subscription in subscribers {
            self.send_to_client(&subscription.client_id, message.clone()).await?;
        }
        
        Ok(())
    }
    
    async fn send_to_client(&self, client_id: &str, message: ServerMessage) -> Result<()> {
        let clients = self.clients.read().await;
        if let Some(client) = clients.get(client_id) {
            client.connection.send(message).await
                .map_err(|e| anyhow!("Failed to send message: {}", e))?;
        }
        Ok(())
    }
    
    async fn send_error(
        &self,
        client_id: &str,
        subscription_id: Option<String>,
        code: ErrorCode,
        message: String,
        recoverable: bool,
        retry_after: Option<u32>,
    ) -> Result<()> {
        self.send_to_client(client_id, ServerMessage::Error {
            subscription_id,
            code,
            message,
            recoverable,
            retry_after,
        }).await
    }
}
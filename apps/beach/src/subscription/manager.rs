use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, RwLock};
use anyhow::{Result, anyhow};

use crate::protocol::{
    ClientMessage, ServerMessage, ViewMode, ViewPosition, Dimensions,
    ErrorCode, SubscriptionStatus, NotificationType, SubscriptionInfo
};
use crate::server::terminal_state::{Grid, GridDelta, TerminalStateTracker};
use crate::transport::Transport;

use super::{Subscription, SubscriptionId, ClientId};

/// Manages all client subscriptions and message routing
pub struct SubscriptionManager<T: Transport + Send + 'static> {
    subscriptions: Arc<RwLock<HashMap<SubscriptionId, Subscription>>>,
    clients: Arc<RwLock<HashMap<ClientId, Vec<SubscriptionId>>>>,
    terminal_tracker: Arc<Mutex<TerminalStateTracker>>,
    transport: T,
}

impl<T: Transport + Send + 'static> SubscriptionManager<T> {
    pub fn new(transport: T, terminal_tracker: Arc<Mutex<TerminalStateTracker>>) -> Self {
        Self {
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            clients: Arc::new(RwLock::new(HashMap::new())),
            terminal_tracker,
            transport,
        }
    }
    
    /// Add a new subscription
    pub async fn add_subscription(
        &self,
        subscription_id: SubscriptionId,
        client_id: ClientId,
        dimensions: Dimensions,
        mode: ViewMode,
        position: Option<ViewPosition>,
        connection: mpsc::Sender<ServerMessage>,
        is_controlling: bool,
    ) -> Result<()> {
        let subscription = Subscription::new(
            subscription_id.clone(),
            client_id.clone(),
            dimensions,
            mode,
            position,
            connection,
            is_controlling,
            self.terminal_tracker.clone(),
        );
        
        // Send subscription acknowledgment
        subscription.send(ServerMessage::SubscriptionAck {
            subscription_id: subscription_id.clone(),
            status: SubscriptionStatus::Active,
            shared_with: None,
        }).await?;
        
        // Send initial snapshot
        let snapshot = subscription.create_snapshot()?;
        subscription.send(ServerMessage::Snapshot {
            subscription_id: subscription_id.clone(),
            sequence: 0,
            grid: snapshot,
            timestamp: chrono::Utc::now().timestamp(),
            checksum: 0,
        }).await?;
        
        // Store subscription
        let mut subscriptions = self.subscriptions.write().await;
        subscriptions.insert(subscription_id.clone(), subscription);
        
        // Track client subscriptions
        let mut clients = self.clients.write().await;
        clients.entry(client_id).or_insert_with(Vec::new).push(subscription_id);
        
        Ok(())
    }
    
    /// Remove a subscription
    pub async fn remove_subscription(&self, subscription_id: &SubscriptionId) -> Result<()> {
        let mut subscriptions = self.subscriptions.write().await;
        if let Some(subscription) = subscriptions.remove(subscription_id) {
            // Remove from client tracking
            let mut clients = self.clients.write().await;
            if let Some(client_subs) = clients.get_mut(&subscription.client_id) {
                client_subs.retain(|id| id != subscription_id);
                if client_subs.is_empty() {
                    clients.remove(&subscription.client_id);
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
            for id in subscription_ids {
                subscriptions.remove(&id);
            }
        }
        Ok(())
    }
    
    /// Handle a message from a client
    pub async fn handle_client_message(&self, client_id: ClientId, message: ClientMessage) -> Result<()> {
        match message {
            ClientMessage::Subscribe { subscription_id, dimensions, mode, position, .. } => {
                // This should be handled at a higher level that has access to the connection
                // For now, return an error
                return Err(anyhow!("Subscribe should be handled at session level"));
            }
            
            ClientMessage::ModifySubscription { subscription_id, dimensions, mode, position } => {
                let mut subscriptions = self.subscriptions.write().await;
                if let Some(subscription) = subscriptions.get_mut(&subscription_id) {
                    if let Some(dims) = dimensions {
                        subscription.update_dimensions(dims);
                    }
                    if let Some(m) = mode {
                        subscription.update_view(m, position);
                    }
                    
                    // Send updated snapshot
                    let snapshot = subscription.create_snapshot()?;
                    let sequence = subscription.next_sequence();
                    subscription.send(ServerMessage::Snapshot {
                        subscription_id: subscription_id.clone(),
                        sequence,
                        grid: snapshot,
                        timestamp: chrono::Utc::now().timestamp(),
                        checksum: 0,
                    }).await?;
                }
            }
            
            ClientMessage::Unsubscribe { subscription_id } => {
                self.remove_subscription(&subscription_id).await?;
            }
            
            ClientMessage::TerminalInput { data, .. } => {
                // Check if client is allowed to send input
                let subscriptions = self.subscriptions.read().await;
                let is_controlling = subscriptions.values()
                    .any(|s| s.client_id == client_id && s.is_controlling);
                    
                if !is_controlling {
                    // Find a subscription for this client to send error
                    if let Some(subscription) = subscriptions.values()
                        .find(|s| s.client_id == client_id) {
                        subscription.send(ServerMessage::Error {
                            subscription_id: Some(subscription.id.clone()),
                            code: ErrorCode::INPUT_NOT_ALLOWED,
                            message: "Client does not have input permissions".to_string(),
                            recoverable: true,
                            retry_after: None,
                        }).await?;
                    }
                    return Ok(());
                }
                
                // Forward input to terminal
                self.transport.send(&data).await?;
            }
            
            ClientMessage::RequestState { subscription_id, .. } => {
                let subscriptions = self.subscriptions.read().await;
                if let Some(subscription) = subscriptions.get(&subscription_id) {
                    let snapshot = subscription.create_snapshot()?;
                    let sequence = subscription.current_sequence;
                    subscription.send(ServerMessage::Snapshot {
                        subscription_id,
                        sequence,
                        grid: snapshot,
                        timestamp: chrono::Utc::now().timestamp(),
                        checksum: 0,
                    }).await?;
                }
            }
            
            ClientMessage::Acknowledge { .. } => {
                // Simple implementation - just track acknowledgment
            }
            
            ClientMessage::Control { .. } => {
                // Handle control messages if needed
            }
            
            ClientMessage::Ping { timestamp, subscriptions: sub_ids } => {
                let subscriptions = self.subscriptions.read().await;
                let mut subscription_info = HashMap::new();
                
                for sub_id in sub_ids {
                    if let Some(subscription) = subscriptions.get(&sub_id) {
                        subscription_info.insert(sub_id, SubscriptionInfo {
                            sequence: subscription.last_sequence_acked,
                            mode: subscription.mode.clone(),
                        });
                    }
                }
                
                // Find any subscription for this client to send pong
                if let Some(subscription) = subscriptions.values()
                    .find(|s| s.client_id == client_id) {
                    subscription.send(ServerMessage::Pong {
                        timestamp,
                        server_sequence: subscription.current_sequence,
                        subscriptions: subscription_info,
                    }).await?;
                }
            }
        }
        
        Ok(())
    }
    
    /// Broadcast a terminal update to all subscriptions
    pub async fn broadcast_terminal_update(&self, delta: GridDelta) -> Result<()> {
        let mut subscriptions = self.subscriptions.write().await;
        
        for subscription in subscriptions.values_mut() {
            let sequence = subscription.next_sequence();
            subscription.send(ServerMessage::Delta {
                subscription_id: subscription.id.clone(),
                sequence,
                changes: delta.clone(),
                timestamp: chrono::Utc::now().timestamp(),
            }).await?;
        }
        
        Ok(())
    }
    
    /// Send a notification to all subscriptions
    pub async fn broadcast_notification(&self, notification_type: NotificationType, details: Option<serde_json::Value>) -> Result<()> {
        let subscriptions = self.subscriptions.read().await;
        
        for subscription in subscriptions.values() {
            subscription.send(ServerMessage::Notify {
                notification_type: notification_type.clone(),
                subscription_id: Some(subscription.id.clone()),
                details: details.clone(),
            }).await?;
        }
        
        Ok(())
    }
    
    /// Get subscription info for monitoring
    pub async fn get_subscription_count(&self) -> usize {
        self.subscriptions.read().await.len()
    }
    
    /// Get client count
    pub async fn get_client_count(&self) -> usize {
        self.clients.read().await.len()
    }
}
use std::collections::HashMap;
use crate::protocol::{ViewMode, ViewPosition, Dimensions};
use super::view_registry::{ViewKey, ViewId, SubscriptionId, ClientId};

#[derive(Debug, Clone)]
pub struct Subscription {
    pub id: SubscriptionId,
    pub client_id: ClientId,
    pub view_id: ViewId,
    pub view_key: ViewKey,
    pub compression: Option<crate::protocol::CompressionType>,
    pub created_at: i64,
    pub last_sequence_acked: u64,
    pub pending_ack: bool,
}

impl Subscription {
    pub fn new(
        id: SubscriptionId,
        client_id: ClientId,
        view_id: ViewId,
        view_key: ViewKey,
        compression: Option<crate::protocol::CompressionType>,
    ) -> Self {
        Self {
            id,
            client_id,
            view_id,
            view_key,
            compression,
            created_at: chrono::Utc::now().timestamp(),
            last_sequence_acked: 0,
            pending_ack: false,
        }
    }
    
    pub fn update_view(&mut self, view_id: ViewId, view_key: ViewKey) {
        self.view_id = view_id;
        self.view_key = view_key;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolStatus {
    Created,
    Joined(usize),
}

pub struct SubscriptionPool {
    subscriptions: HashMap<SubscriptionId, Subscription>,
    client_subscriptions: HashMap<ClientId, Vec<SubscriptionId>>,
    view_subscribers: HashMap<ViewId, Vec<SubscriptionId>>,
}

impl SubscriptionPool {
    pub fn new() -> Self {
        Self {
            subscriptions: HashMap::new(),
            client_subscriptions: HashMap::new(),
            view_subscribers: HashMap::new(),
        }
    }
    
    pub fn add_subscription(&mut self, subscription: Subscription) -> PoolStatus {
        let subscription_id = subscription.id.clone();
        let client_id = subscription.client_id.clone();
        let view_id = subscription.view_id.clone();
        
        self.client_subscriptions
            .entry(client_id)
            .or_insert_with(Vec::new)
            .push(subscription_id.clone());
        
        let subscribers = self.view_subscribers
            .entry(view_id.clone())
            .or_insert_with(Vec::new);
        
        subscribers.push(subscription_id.clone());
        let pool_size = subscribers.len();
        
        self.subscriptions.insert(subscription_id, subscription);
        
        if pool_size == 1 {
            PoolStatus::Created
        } else {
            PoolStatus::Joined(pool_size)
        }
    }
    
    pub fn remove_subscription(&mut self, subscription_id: &str) -> Option<Subscription> {
        if let Some(subscription) = self.subscriptions.remove(subscription_id) {
            if let Some(client_subs) = self.client_subscriptions.get_mut(&subscription.client_id) {
                client_subs.retain(|id| id != subscription_id);
                if client_subs.is_empty() {
                    self.client_subscriptions.remove(&subscription.client_id);
                }
            }
            
            if let Some(view_subs) = self.view_subscribers.get_mut(&subscription.view_id) {
                view_subs.retain(|id| id != subscription_id);
                if view_subs.is_empty() {
                    self.view_subscribers.remove(&subscription.view_id);
                }
            }
            
            Some(subscription)
        } else {
            None
        }
    }
    
    pub fn get_subscription(&self, subscription_id: &str) -> Option<&Subscription> {
        self.subscriptions.get(subscription_id)
    }
    
    pub fn get_subscription_mut(&mut self, subscription_id: &str) -> Option<&mut Subscription> {
        self.subscriptions.get_mut(subscription_id)
    }
    
    pub fn get_client_subscriptions(&self, client_id: &str) -> Vec<&Subscription> {
        self.client_subscriptions
            .get(client_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.subscriptions.get(id))
                    .collect()
            })
            .unwrap_or_else(Vec::new)
    }
    
    pub fn get_view_subscribers(&self, view_id: &str) -> Vec<&Subscription> {
        self.view_subscribers
            .get(view_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.subscriptions.get(id))
                    .collect()
            })
            .unwrap_or_else(Vec::new)
    }
    
    pub fn get_view_subscriber_ids(&self, view_id: &str) -> Vec<SubscriptionId> {
        self.view_subscribers
            .get(view_id)
            .cloned()
            .unwrap_or_else(Vec::new)
    }
    
    pub fn move_subscription_to_view(&mut self, subscription_id: &str, new_view_id: ViewId, new_view_key: ViewKey) -> bool {
        if let Some(subscription) = self.subscriptions.get_mut(subscription_id) {
            let old_view_id = subscription.view_id.clone();
            
            if let Some(old_subs) = self.view_subscribers.get_mut(&old_view_id) {
                old_subs.retain(|id| id != subscription_id);
                if old_subs.is_empty() {
                    self.view_subscribers.remove(&old_view_id);
                }
            }
            
            self.view_subscribers
                .entry(new_view_id.clone())
                .or_insert_with(Vec::new)
                .push(subscription_id.to_string());
            
            subscription.update_view(new_view_id, new_view_key);
            true
        } else {
            false
        }
    }
    
    pub fn remove_client(&mut self, client_id: &str) -> Vec<Subscription> {
        let subscription_ids = self.client_subscriptions
            .remove(client_id)
            .unwrap_or_else(Vec::new);
        
        subscription_ids.into_iter()
            .filter_map(|id| self.remove_subscription(&id))
            .collect()
    }
    
    pub fn subscription_count(&self) -> usize {
        self.subscriptions.len()
    }
    
    pub fn client_count(&self) -> usize {
        self.client_subscriptions.len()
    }
}
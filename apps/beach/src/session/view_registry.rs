use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use crate::protocol::{ViewMode, ViewPosition, Dimensions};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewKey {
    pub dimensions: Dimensions,
    pub mode: ViewMode,
    pub position: Option<ViewPosition>,
}

impl ViewKey {
    pub fn new(dimensions: Dimensions, mode: ViewMode, position: Option<ViewPosition>) -> Self {
        Self {
            dimensions,
            mode,
            position,
        }
    }
    
    pub fn realtime(width: u16, height: u16) -> Self {
        Self {
            dimensions: Dimensions { width, height },
            mode: ViewMode::Realtime,
            position: None,
        }
    }
    
    pub fn historical(width: u16, height: u16, timestamp: i64) -> Self {
        Self {
            dimensions: Dimensions { width, height },
            mode: ViewMode::Historical,
            position: Some(ViewPosition {
                time: Some(timestamp),
                line: None,
                offset: None,
            }),
        }
    }
    
    pub fn anchored(width: u16, height: u16, line: u64) -> Self {
        Self {
            dimensions: Dimensions { width, height },
            mode: ViewMode::Anchored,
            position: Some(ViewPosition {
                time: None,
                line: Some(line),
                offset: None,
            }),
        }
    }
}

impl Hash for ViewKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.dimensions.width.hash(state);
        self.dimensions.height.hash(state);
        self.mode.hash(state);
        
        if let Some(pos) = &self.position {
            pos.time.hash(state);
            pos.line.hash(state);
            pos.offset.hash(state);
        }
    }
}

pub type ViewId = String;
pub type SubscriptionId = String;
pub type ClientId = String;

#[derive(Debug, Clone)]
pub struct ViewInfo {
    pub view_id: ViewId,
    pub view_key: ViewKey,
    pub current_sequence: u64,
    pub subscribers: HashSet<SubscriptionId>,
    pub last_checksum: u32,
    pub created_at: i64,
}

impl ViewInfo {
    pub fn new(view_id: ViewId, view_key: ViewKey) -> Self {
        Self {
            view_id,
            view_key,
            current_sequence: 0,
            subscribers: HashSet::new(),
            last_checksum: 0,
            created_at: chrono::Utc::now().timestamp(),
        }
    }
    
    pub fn add_subscriber(&mut self, subscription_id: SubscriptionId) {
        self.subscribers.insert(subscription_id);
    }
    
    pub fn remove_subscriber(&mut self, subscription_id: &str) -> bool {
        self.subscribers.remove(subscription_id)
    }
    
    pub fn is_empty(&self) -> bool {
        self.subscribers.is_empty()
    }
    
    pub fn subscriber_count(&self) -> usize {
        self.subscribers.len()
    }
    
    pub fn increment_sequence(&mut self) -> u64 {
        self.current_sequence += 1;
        self.current_sequence
    }
}

pub struct ViewRegistry {
    views_by_key: HashMap<ViewKey, ViewInfo>,
    views_by_id: HashMap<ViewId, ViewKey>,
    next_view_id: u64,
}

impl ViewRegistry {
    pub fn new() -> Self {
        Self {
            views_by_key: HashMap::new(),
            views_by_id: HashMap::new(),
            next_view_id: 1,
        }
    }
    
    pub fn find_or_create_view(&mut self, view_key: ViewKey) -> (ViewId, bool) {
        if let Some(view_info) = self.views_by_key.get(&view_key) {
            (view_info.view_id.clone(), false)
        } else {
            let view_id = format!("view_{}", self.next_view_id);
            self.next_view_id += 1;
            
            let view_info = ViewInfo::new(view_id.clone(), view_key.clone());
            self.views_by_key.insert(view_key.clone(), view_info);
            self.views_by_id.insert(view_id.clone(), view_key);
            
            (view_id, true)
        }
    }
    
    pub fn get_view(&self, view_id: &str) -> Option<&ViewInfo> {
        self.views_by_id.get(view_id)
            .and_then(|key| self.views_by_key.get(key))
    }
    
    pub fn get_view_mut(&mut self, view_id: &str) -> Option<&mut ViewInfo> {
        let key = self.views_by_id.get(view_id)?;
        self.views_by_key.get_mut(key)
    }
    
    pub fn get_view_by_key(&self, view_key: &ViewKey) -> Option<&ViewInfo> {
        self.views_by_key.get(view_key)
    }
    
    pub fn remove_view(&mut self, view_id: &str) -> Option<ViewInfo> {
        if let Some(view_key) = self.views_by_id.remove(view_id) {
            self.views_by_key.remove(&view_key)
        } else {
            None
        }
    }
    
    pub fn all_views(&self) -> impl Iterator<Item = &ViewInfo> {
        self.views_by_key.values()
    }
    
    pub fn view_count(&self) -> usize {
        self.views_by_key.len()
    }
}
use std::sync::Arc;


pub enum SyncClientMessage {
    RequestCreateSubscription,
    RequestDeleteSubscription,
}

pub enum SyncServerMessage {
    SubscriptionCreated {
        // most recent order_key that was stored in cache
        max_order_key: u64,
        // optional initial state that can be returned to populate a client cache
        snapshot: Some(Vec<CacheItem>),
    },
    Snapshot {
        // most recent order_key that was stored in cache
        max_order_key: u64,
        // optional initial state that can be returned to populate a client cache
        snapshot: Some(Vec<CacheItem>),
    },
    Delta,
}

pub enum CacheSyncProtocol {
    ClientMessage(ClientMessage),
    AckSubscriptionCreated {
        byte_seq_num: u64,
    },
    RequestLocationRange,
    Delta,
    Snapshot,
}

pub trait ServerCacheSynchronizer<T: ServerCache> {
    fn new(cache: Arc<T>) -> Self;
    async fn start(&self);
    async fn stop(&self);
}

// Responsible for keeping subscribers in sync with a server cache
pub struct ServerCacheSynchronizer<T: ServerCache> {
    session_id: str,
    subscribers: Vec<CacheSubscription>,
    cache: Arc<T>,
}

// Responsible for keeping a client cache in sync with a server cache
pub struct ClientCacheSynchronizer<T: ClientCache> {
    session_id: str,
}

pub trait CacheSyncItem {
    // order_key represents the order in which items should be applied to a cache.
    // if two items are in contention to update the same cache cell, the item with
    // the greatest order_key will be chosen.
    fn get_order_key(&self) -> u64;
}

pub trait CacheItemDiff {
    fn get_cache_location(&self) -> CacheLocation;
}

// ServerCachePublisher is responsible for broadcasting updates to the server cache to client caches
pub trait ServerCachePublisher {
    fn new(session_id: str, cache: ServerCache) -> Self;
    async fn publish(diffs: Array<Diff>);
}

pub trait ClientCacheSubscriber {
    fn new(session_id: str, cache: ClientCache) -> Self;
    async fn consume(diffs: Array<Diff>);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Critical = 5,
    High = 4,
    Medium = 3,
    Low = 2,
}

pub trait CacheSync {



    fn get_session_id(&self) -> str;
    fn get_subscribers(&self) -> Arr<CacheSubscriber>;
    fn add_subscriber(&self, subscriber: CacheSubscriber);
    fn remove_subscriber(&self, subscriber: CacheSubscriber);
}

pub trait CacheSubscriber<C: ServerCache> {
    async fn sync(&self, cache: C);
}

pub struct ServerSync<ServerCache> {
    subscribers: Arr<Arc<CacheSubscriber>>,
}

impl CacheSync for ServerSync {

}

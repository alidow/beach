
// A SequentialCacheItem is a cached item that has a seq_num associated with it.
pub trait SequentialCacheItem {
    type SeqType: Copy + Ord = u32;
    fn seqnum(&self) -> SeqType;
    fn set_seqnum(&mut self, val: SeqType);
}

// A sequential cache stores ordered items, where the item with the higest seq_num
// is the most recent and "correct" item.
pub trait SequentialCache {
    type SeqType: Copy + Ord = u32;
    type IncrFuture<'a>: Future<Output = SeqType> + 'a 
        where Self: 'a;
    // get the current seq_num for the cache. this is the most recent (and thus greatest)
    // seqnum belonging to an item that has been written to the cache.
    fn get_seqnum(&self) -> Self::SeqType;

    // increment the seq_num for the cache
    fn incr_seqnum<'a>(&'a mut self, increment: Option<Self::SeqType>) -> Self::IncrFuture<'a>;
}

pub trait CartesianCache {
    type Coords: ?Sized = [u32];
    type CacheItemType = SequentialCacheItem;
    fn get_range(&self, from: &Self::Coords, to: &Self::Coords) -> &[Self::CacheItemType];
}

pub trait ServerCacheHandler {
    type Coords: ?Sized = [u32];
    type CacheItemType = SequentialCacheItem;
    type OnWriteFuture<'a>: Future<Output = ()> + 'a
        where Self: 'a;
    fn on_write<'a>(&'a self, item: &Self::SequentialCacheItem, coords: &Self::Coords) -> Self::OnWriteFuture<'a>;
}

pub trait ServerCache : SequentialCache + CartesianCache {
    type HandlerType: ServerCacheHandler;
    type WriteFuture: Future<Output = bool> + 'a
        where Self: 'a;

    fn new(handler: &Option(ServerCacheHandler)) -> Self;

    // returns true if write succeeded
    fn write_if_newer(&self, item: SequentialCacheItem, coords: Vec<u32>) -> bool;
}

pub trait ClientCache<L> : SequentialCache + CartesianCache {
    // returns true if write succeeded
    async fn try_write(&self, item: SyncItem, location: L) -> bool;
}
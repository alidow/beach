//! Generic synchronization primitives for cache replication.
//!
//! The sync layer is designed to be transport agnostic.  A `ServerSynchronizer`
//! exposes chunked snapshot and delta batches that higher level transports can
//! stream to remote peers.  The only requirement is that the cache domain can
//! provide ordered updates (`SyncUpdate`).

use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use crate::cache::Seq;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Watermark(pub Seq);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PriorityLane {
    /// Highest priority. Used to stream the currently visible region first.
    Foreground,
    /// Medium priority. Used to backfill rows that recently changed.
    Recent,
    /// Lowest priority. Used to trickle the remaining historical content.
    History,
}

impl PriorityLane {
    #[inline]
    fn as_index(self) -> usize {
        match self {
            PriorityLane::Foreground => 0,
            PriorityLane::Recent => 1,
            PriorityLane::History => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneBudget {
    pub lane: PriorityLane,
    /// Maximum number of updates that should be included in a snapshot chunk
    /// for this lane.
    pub max_updates: usize,
}

impl LaneBudget {
    pub const fn new(lane: PriorityLane, max_updates: usize) -> Self {
        Self { lane, max_updates }
    }
}

#[derive(Debug, Clone)]
pub struct SyncConfig {
    pub snapshot_budgets: Vec<LaneBudget>,
    /// Upper bound on the number of updates emitted in a delta batch.
    pub delta_budget: usize,
    pub heartbeat_interval: Duration,
    /// Number of rows prioritized in the initial snapshot lane.
    pub initial_snapshot_lines: usize,
}

impl SyncConfig {
    pub fn budget_for(&self, lane: PriorityLane) -> usize {
        self.snapshot_budgets
            .iter()
            .find(|entry| entry.lane == lane)
            .map(|entry| entry.max_updates)
            .unwrap_or(0)
    }
}

const DEFAULT_INITIAL_SNAPSHOT_LINES: usize = 500;

impl Default for SyncConfig {
    fn default() -> Self {
        let initial = DEFAULT_INITIAL_SNAPSHOT_LINES;
        Self {
            snapshot_budgets: vec![
                LaneBudget::new(PriorityLane::Foreground, initial),
                LaneBudget::new(PriorityLane::Recent, initial),
                LaneBudget::new(PriorityLane::History, initial),
            ],
            delta_budget: 512,
            heartbeat_interval: Duration::from_millis(250),
            initial_snapshot_lines: initial,
        }
    }
}

pub trait SyncUpdate: Clone + Send + Sync + 'static {
    fn seq(&self) -> Seq;
    /// Optional weighting function. Defaults to `1` so the budgets operate on
    /// “number of updates”. Implementations can override to let budgets operate
    /// on bytes or cells instead.
    fn cost(&self) -> usize {
        1
    }
}

#[derive(Debug, Clone)]
pub struct SnapshotSlice<U: SyncUpdate> {
    pub updates: Vec<U>,
    pub watermark: Watermark,
    pub has_more: bool,
}

#[derive(Debug, Clone)]
pub struct SnapshotChunk<U: SyncUpdate> {
    pub subscription_id: SubscriptionId,
    pub lane: PriorityLane,
    pub watermark: Watermark,
    pub has_more: bool,
    pub updates: Vec<U>,
}

impl<U: SyncUpdate> SnapshotChunk<U> {
    pub fn from_slice(
        subscription_id: SubscriptionId,
        lane: PriorityLane,
        slice: SnapshotSlice<U>,
    ) -> Self {
        Self {
            subscription_id,
            lane,
            watermark: slice.watermark,
            has_more: slice.has_more,
            updates: slice.updates,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DeltaSlice<U: SyncUpdate> {
    pub updates: Vec<U>,
    pub watermark: Watermark,
    pub has_more: bool,
}

#[derive(Debug, Clone)]
pub struct DeltaBatch<U: SyncUpdate> {
    pub subscription_id: SubscriptionId,
    pub watermark: Watermark,
    pub has_more: bool,
    pub updates: Vec<U>,
}

impl<U: SyncUpdate> DeltaBatch<U> {
    pub fn from_slice(subscription_id: SubscriptionId, slice: DeltaSlice<U>) -> Self {
        Self {
            subscription_id,
            watermark: slice.watermark,
            has_more: slice.has_more,
            updates: slice.updates,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClientHello {
    pub subscription_id: SubscriptionId,
    pub last_seen: Option<Watermark>,
    /// Request that the server sends backlog/history chunks after the
    /// foreground snapshot is complete.
    pub request_history: bool,
}

#[derive(Debug, Clone)]
pub struct ServerHello {
    pub subscription_id: SubscriptionId,
    pub max_seq: Watermark,
    pub config: SyncConfig,
}

#[derive(Debug, Clone)]
pub struct Ack {
    pub subscription_id: SubscriptionId,
    pub watermark: Watermark,
}

#[derive(Debug, Clone)]
pub enum ClientMessage {
    Hello(ClientHello),
    Ack(Ack),
    RequestSnapshot { lane: PriorityLane, budget: usize },
    RequestResync { since: Watermark },
    Heartbeat,
    Pause,
    Resume,
}

#[derive(Debug, Clone)]
pub enum ServerMessage<U: SyncUpdate> {
    Hello(ServerHello),
    Snapshot(SnapshotChunk<U>),
    Delta(DeltaBatch<U>),
    SnapshotComplete {
        subscription_id: SubscriptionId,
        lane: PriorityLane,
    },
    Heartbeat(Watermark),
}

pub trait SnapshotSource<U: SyncUpdate>: Send + Sync {
    type Cursor: Default + Send;

    fn max_seq(&self) -> Seq;
    fn reset_lane(&self, cursor: &mut Self::Cursor, lane: PriorityLane);
    fn next_slice(
        &self,
        cursor: &mut Self::Cursor,
        lane: PriorityLane,
        budget: usize,
    ) -> Option<SnapshotSlice<U>>;
}

pub trait DeltaSource<U: SyncUpdate>: Send + Sync {
    fn next_delta(&self, since: Seq, budget: usize) -> Option<DeltaSlice<U>>;
}

#[derive(Debug)]
pub struct ServerSynchronizer<S, U>
where
    S: SnapshotSource<U> + DeltaSource<U>,
    U: SyncUpdate,
{
    source: Arc<S>,
    cursor: S::Cursor,
    lane_initialized: [bool; 3],
    lane_complete: [bool; 3],
    config: SyncConfig,
    _marker: PhantomData<U>,
}

impl<S, U> ServerSynchronizer<S, U>
where
    S: SnapshotSource<U> + DeltaSource<U>,
    U: SyncUpdate,
{
    pub fn new(source: Arc<S>, config: SyncConfig) -> Self {
        Self {
            source,
            cursor: Default::default(),
            lane_initialized: [false, false, false],
            lane_complete: [false, false, false],
            config,
            _marker: PhantomData,
        }
    }

    pub fn hello(&self, subscription_id: SubscriptionId) -> ServerHello {
        ServerHello {
            subscription_id,
            max_seq: Watermark(self.source.max_seq()),
            config: self.config.clone(),
        }
    }

    pub fn snapshot_chunk(
        &mut self,
        subscription_id: SubscriptionId,
        lane: PriorityLane,
    ) -> Option<SnapshotChunk<U>> {
        let budget = self.config.budget_for(lane);
        if budget == 0 {
            return None;
        }
        let idx = lane.as_index();
        if self.lane_complete[idx] {
            return None;
        }
        if !self.lane_initialized[idx] {
            SnapshotSource::reset_lane(&*self.source, &mut self.cursor, lane);
            self.lane_initialized[idx] = true;
        }
        let slice = match SnapshotSource::next_slice(&*self.source, &mut self.cursor, lane, budget)
        {
            Some(slice) => slice,
            None => {
                self.lane_initialized[idx] = false;
                self.lane_complete[idx] = true;
                return None;
            }
        };
        let has_more = slice.has_more;
        if !has_more {
            self.lane_initialized[idx] = false;
            self.lane_complete[idx] = true;
        }
        Some(SnapshotChunk::from_slice(subscription_id, lane, slice))
    }

    pub fn delta_batch(
        &self,
        subscription_id: SubscriptionId,
        since: Seq,
    ) -> Option<DeltaBatch<U>> {
        let budget = self.config.delta_budget;
        if budget == 0 {
            return None;
        }
        let slice = DeltaSource::next_delta(&*self.source, since, budget)?;
        Some(DeltaBatch::from_slice(subscription_id, slice))
    }

    pub fn config(&self) -> &SyncConfig {
        &self.config
    }

    pub fn reset(&mut self)
    where
        S::Cursor: Default,
    {
        self.cursor = Default::default();
        self.lane_initialized = [false; 3];
        self.lane_complete = [false; 3];
    }
}

pub mod terminal;

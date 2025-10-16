//! Generic cache traits for a 2D grid of opaque packed payloads.
//!
//! The [`GridCache`] trait is intentionally lightweight so the same storage
//! primitives can back both the terminal cache and any future pixel/metadata
//! layers. Typical usage looks like:
//!
//! ```
//! # use beach::cache::{GridCache, CellSnapshot, WriteOutcome};
//! # fn demo(grid: &dyn GridCache) {
//! let payload = 42u64;
//! let seq = 10;
//! assert_eq!(
//!     grid.write_cell_if_newer(0, 0, seq, payload).unwrap(),
//!     WriteOutcome::Written,
//! );
//! if let Some(CellSnapshot { payload, seq }) = grid.get_cell_relaxed(0, 0) {
//!     assert_eq!(payload, 42);
//!     assert_eq!(seq, 10);
//! }
//! # }
//! ```
//!
//! Design goals:
//! - High-throughput bulk updates (rows/rectangles)
//! - Low memory overhead per cell
//! - Simple, synchronous APIs

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Written,
    SkippedOlder,
    SkippedEqual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteError {
    CoordOutOfBounds,
}

/// Monotonic sequence number used for conflict resolution (e.g., terminal byte index)
pub type Seq = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellSnapshot {
    pub payload: u64,
    pub seq: Seq,
}

impl CellSnapshot {
    #[inline]
    pub fn new(payload: u64, seq: Seq) -> Self {
        Self { payload, seq }
    }
}

/// A 2D grid cache interface operating on packed payloads.
/// The payload is an opaque `u64` chosen by the caller (e.g., pixels, terminal cells, etc.).
pub trait GridCache {
    /// Returns (rows, cols)
    fn dims(&self) -> (usize, usize);

    /// Write a single cell if `seq` is newer than the stored sequence.
    fn write_cell_if_newer(
        &self,
        row: usize,
        col: usize,
        seq: Seq,
        payload: u64,
    ) -> Result<WriteOutcome, WriteError>;

    /// Fill a rectangular region [row0,row1) x [col0,col1) with `payload` if `seq` is newer.
    /// Returns (written_count, skipped_count).
    fn fill_rect_if_newer(
        &self,
        row0: usize,
        col0: usize,
        row1: usize,
        col1: usize,
        seq: Seq,
        payload: u64,
    ) -> Result<(usize, usize), WriteError>;

    /// Snapshot a row's packed payloads into `out`. `out.len()` must equal `cols`.
    fn snapshot_row_into(&self, row: usize, out: &mut [u64]) -> Result<(), WriteError>;

    /// Get a single cell's packed payload and seq (relaxed snapshot).
    fn get_cell_relaxed(&self, row: usize, col: usize) -> Option<CellSnapshot>;
}

// Re-export the generic grid implementation
pub mod grid;
pub mod terminal;

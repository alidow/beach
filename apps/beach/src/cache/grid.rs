use std::mem;
use std::sync::atomic::{
    AtomicU64,
    Ordering::{Acquire, Release},
};

use super::{CellSnapshot, GridCache, Seq, WriteError, WriteOutcome};

#[derive(Debug)]
struct ActiveRow {
    payloads: Vec<AtomicU64>,
    seqs: Vec<AtomicU64>,
}

impl ActiveRow {
    fn new(cols: usize, default_payload: u64, default_seq: u64) -> Self {
        let payloads = (0..cols).map(|_| AtomicU64::new(default_payload)).collect();
        let seqs = (0..cols).map(|_| AtomicU64::new(default_seq)).collect();
        Self { payloads, seqs }
    }

    #[inline]
    fn cols(&self) -> usize {
        self.payloads.len()
    }
}

#[derive(Debug)]
enum RowState {
    Active(ActiveRow),
    Frozen { payloads: Vec<u64>, seqs: Vec<Seq> },
}

/// Lock-free grid backed by atomics, tuned for high write volumes and occasional
/// read snapshots. Rows can be frozen/thawed to cheaply archive scrollback while
/// keeping recent rows hot.
///
/// # Examples
///
/// ```
/// # use beach::cache::grid::AtomicGrid;
/// # use beach::cache::{GridCache, WriteOutcome};
/// let grid = AtomicGrid::new(2, 2, 0, 0);
/// assert_eq!(grid.write_cell_if_newer(0, 0, 1, 42).unwrap(), WriteOutcome::Written);
/// let snapshot = grid.get_cell_relaxed(0, 0).unwrap();
/// assert_eq!(snapshot.payload, 42);
/// assert_eq!(snapshot.seq, 1);
/// ```
pub struct AtomicGrid {
    rows: Vec<RowState>,
    cols: usize,
}

impl AtomicGrid {
    pub fn new(rows: usize, cols: usize, default_payload: u64, default_seq: Seq) -> Self {
        let mut v = Vec::with_capacity(rows);
        for _ in 0..rows {
            v.push(RowState::Active(ActiveRow::new(
                cols,
                default_payload,
                default_seq,
            )));
        }
        Self { rows: v, cols }
    }

    /// Freeze a row (convert active atomics to immutable payloads). Requires exclusive access.
    pub fn freeze_row(&mut self, row: usize) -> Result<(), WriteError> {
        if row >= self.rows.len() {
            return Err(WriteError::CoordOutOfBounds);
        }
        let current = mem::replace(
            &mut self.rows[row],
            RowState::Frozen {
                payloads: Vec::new(),
                seqs: Vec::new(),
            },
        );
        self.rows[row] = match current {
            RowState::Active(r) => {
                let mut payloads = Vec::with_capacity(r.cols());
                payloads.extend(
                    r.payloads
                        .iter()
                        .take(r.cols())
                        .map(|cell| cell.load(Acquire)),
                );
                let mut seqs = Vec::with_capacity(r.cols());
                seqs.extend(r.seqs.iter().take(r.cols()).map(|cell| cell.load(Acquire)));
                RowState::Frozen { payloads, seqs }
            }
            RowState::Frozen { payloads, seqs } => RowState::Frozen { payloads, seqs },
        };
        Ok(())
    }

    /// Thaw a row back to active (allocates new atomics, restoring payloads + seqs).
    pub fn thaw_row(&mut self, row: usize, seq: Seq) -> Result<(), WriteError> {
        if row >= self.rows.len() {
            return Err(WriteError::CoordOutOfBounds);
        }
        let current = mem::replace(
            &mut self.rows[row],
            RowState::Frozen {
                payloads: Vec::new(),
                seqs: Vec::new(),
            },
        );
        self.rows[row] = match current {
            RowState::Frozen { payloads, seqs } => {
                let cols = payloads.len();
                let active = ActiveRow::new(cols, 0, seq);
                for (slot, value) in active.payloads.iter().zip(&payloads) {
                    slot.store(*value, Release);
                }
                for (slot, value) in active.seqs.iter().zip(&seqs) {
                    slot.store(*value, Release);
                }
                RowState::Active(active)
            }
            RowState::Active(r) => RowState::Active(r),
        };
        Ok(())
    }
}

impl GridCache for AtomicGrid {
    fn dims(&self) -> (usize, usize) {
        (self.rows.len(), self.cols)
    }

    fn write_cell_if_newer(
        &self,
        row: usize,
        col: usize,
        new_seq: Seq,
        payload: u64,
    ) -> Result<WriteOutcome, WriteError> {
        if row >= self.rows.len() {
            return Err(WriteError::CoordOutOfBounds);
        }
        match &self.rows[row] {
            RowState::Active(r) => {
                if col >= r.cols() {
                    return Err(WriteError::CoordOutOfBounds);
                }
                let cur_seq = r.seqs[col].load(Acquire);
                if new_seq < cur_seq {
                    return Ok(WriteOutcome::SkippedOlder);
                }
                if new_seq == cur_seq {
                    return Ok(WriteOutcome::SkippedEqual);
                }
                r.payloads[col].store(payload, Release);
                r.seqs[col].store(new_seq, Release);
                Ok(WriteOutcome::Written)
            }
            RowState::Frozen { .. } => Err(WriteError::CoordOutOfBounds),
        }
    }

    fn fill_rect_if_newer(
        &self,
        row0: usize,
        col0: usize,
        row1: usize,
        col1: usize,
        new_seq: Seq,
        payload: u64,
    ) -> Result<(usize, usize), WriteError> {
        if row0 > row1 || col0 > col1 {
            return Ok((0, 0));
        }
        if row1 > self.rows.len() {
            return Err(WriteError::CoordOutOfBounds);
        }
        let mut written = 0usize;
        let mut skipped = 0usize;
        for row in row0..row1 {
            match &self.rows[row] {
                RowState::Active(r) => {
                    if col1 > r.cols() {
                        return Err(WriteError::CoordOutOfBounds);
                    }
                    for (payload_cell, seq_cell) in
                        r.payloads[col0..col1].iter().zip(&r.seqs[col0..col1])
                    {
                        let cur_seq = seq_cell.load(Acquire);
                        if new_seq <= cur_seq {
                            skipped += 1;
                        } else {
                            payload_cell.store(payload, Release);
                            seq_cell.store(new_seq, Release);
                            written += 1;
                        }
                    }
                }
                RowState::Frozen { .. } => return Err(WriteError::CoordOutOfBounds),
            }
        }
        Ok((written, skipped))
    }

    fn snapshot_row_into(&self, row: usize, out: &mut [u64]) -> Result<(), WriteError> {
        if row >= self.rows.len() {
            return Err(WriteError::CoordOutOfBounds);
        }
        match &self.rows[row] {
            RowState::Active(r) => {
                if out.len() != r.cols() {
                    return Err(WriteError::CoordOutOfBounds);
                }
                for (slot, value) in out.iter_mut().zip(r.payloads.iter()) {
                    *slot = value.load(Acquire);
                }
                Ok(())
            }
            RowState::Frozen { payloads, .. } => {
                if out.len() != payloads.len() {
                    return Err(WriteError::CoordOutOfBounds);
                }
                out.copy_from_slice(&payloads[..]);
                Ok(())
            }
        }
    }

    fn get_cell_relaxed(&self, row: usize, col: usize) -> Option<CellSnapshot> {
        if row >= self.rows.len() {
            return None;
        }
        match &self.rows[row] {
            RowState::Active(r) => {
                if col >= r.cols() {
                    return None;
                }
                let s1 = r.seqs[col].load(Acquire);
                let payload = r.payloads[col].load(Acquire);
                let s2 = r.seqs[col].load(Acquire);
                if s1 != s2 {
                    let payload2 = r.payloads[col].load(Acquire);
                    Some(CellSnapshot::new(payload2, s2))
                } else {
                    Some(CellSnapshot::new(payload, s1))
                }
            }
            RowState::Frozen { payloads, seqs } => {
                if col >= payloads.len() {
                    return None;
                }
                let seq = seqs.get(col).copied().unwrap_or(0);
                Some(CellSnapshot::new(payloads[col], seq))
            }
        }
    }
}

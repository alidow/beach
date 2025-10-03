use std::collections::VecDeque;
use std::convert::TryFrom;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use super::packed::{PackedCell, Style, StyleId, StyleTable, pack_cell, unpack_to_heavy};
use crate::cache::{CellSnapshot, GridCache, Seq, WriteError, WriteOutcome};
use crate::model::terminal::cell::Cell as HeavyCell;

const DEFAULT_HISTORY_LIMIT: usize = 10_000;

#[derive(Clone, Debug)]
pub struct TrimEvent {
    pub start: u64,
    pub count: usize,
}

#[derive(Clone, Debug)]
struct RowEntry {
    absolute: u64,
    cells: Vec<PackedCell>,
    seqs: Vec<Seq>,
    max_seq: Seq,
}

impl RowEntry {
    fn new(absolute: u64, cols: usize, default_cell: PackedCell, default_seq: Seq) -> Self {
        Self {
            absolute,
            cells: vec![default_cell; cols],
            seqs: vec![default_seq; cols],
            max_seq: default_seq,
        }
    }

    fn ensure_cols(&mut self, cols: usize, default_cell: PackedCell, default_seq: Seq) {
        if cols <= self.cells.len() {
            return;
        }
        let current_len = self.cells.len();
        self.cells.resize(cols, default_cell);
        self.seqs.resize(cols, default_seq);
        if current_len == 0 && default_seq > self.max_seq {
            self.max_seq = default_seq;
        }
    }

    fn write_cell_if_newer(
        &mut self,
        col: usize,
        seq: Seq,
        cell: PackedCell,
    ) -> Result<WriteOutcome, WriteError> {
        if col >= self.cells.len() {
            return Err(WriteError::CoordOutOfBounds);
        }
        let current_seq = self.seqs[col];
        if seq < current_seq {
            return Ok(WriteOutcome::SkippedOlder);
        }
        if seq == current_seq {
            return Ok(WriteOutcome::SkippedEqual);
        }
        self.cells[col] = cell;
        self.seqs[col] = seq;
        if seq > self.max_seq {
            self.max_seq = seq;
        }
        Ok(WriteOutcome::Written)
    }

    fn fill_rect_if_newer(
        &mut self,
        col0: usize,
        col1: usize,
        seq: Seq,
        cell: PackedCell,
    ) -> Result<(usize, usize), WriteError> {
        if col0 > col1 || col1 > self.cells.len() {
            return Err(WriteError::CoordOutOfBounds);
        }
        let mut written = 0usize;
        let mut skipped = 0usize;
        for col in col0..col1 {
            let current_seq = self.seqs[col];
            if seq <= current_seq {
                skipped += 1;
                continue;
            }
            self.cells[col] = cell;
            self.seqs[col] = seq;
            written += 1;
        }
        if written > 0 && seq > self.max_seq {
            self.max_seq = seq;
        }
        Ok((written, skipped))
    }

    fn snapshot_row(&self, out: &mut [u64]) -> Result<(), WriteError> {
        if out.len() < self.cells.len() {
            return Err(WriteError::CoordOutOfBounds);
        }
        for (idx, cell) in self.cells.iter().enumerate() {
            out[idx] = (*cell).into();
        }
        Ok(())
    }

    fn cell_snapshot(&self, col: usize) -> Option<CellSnapshot> {
        self.cells
            .get(col)
            .map(|cell| CellSnapshot::new((*cell).into(), self.seqs.get(col).copied().unwrap_or(0)))
    }
}

struct GridInner {
    rows: VecDeque<RowEntry>,
    base: u64,
    cols: usize,
    next_row_id: u64,
}

impl GridInner {
    fn new(rows: usize, cols: usize, default_cell: PackedCell, default_seq: Seq) -> Self {
        let mut entries = VecDeque::with_capacity(rows);
        for absolute in 0..(rows as u64) {
            entries.push_back(RowEntry::new(absolute, cols, default_cell, default_seq));
        }
        Self {
            rows: entries,
            base: 0,
            cols,
            next_row_id: rows as u64,
        }
    }

    fn len(&self) -> usize {
        self.rows.len()
    }

    fn cols(&self) -> usize {
        self.cols
    }

    fn ensure_cols(&mut self, cols: usize, default_cell: PackedCell, default_seq: Seq) {
        if cols <= self.cols {
            return;
        }
        for row in self.rows.iter_mut() {
            row.ensure_cols(cols, default_cell, default_seq);
        }
        self.cols = cols;
    }

    fn ensure_row(
        &mut self,
        absolute: u64,
        default_cell: PackedCell,
        default_seq: Seq,
    ) -> Result<usize, WriteError> {
        if absolute < self.base {
            return Err(WriteError::CoordOutOfBounds);
        }
        while absolute >= self.next_row_id {
            let next_abs = self.next_row_id;
            self.rows.push_back(RowEntry::new(
                next_abs,
                self.cols,
                default_cell,
                default_seq,
            ));
            self.next_row_id = self.next_row_id.saturating_add(1);
        }
        let offset = absolute
            .checked_sub(self.base)
            .ok_or(WriteError::CoordOutOfBounds)?;
        let index = offset as usize;
        if index >= self.rows.len() {
            return Err(WriteError::CoordOutOfBounds);
        }
        Ok(index)
    }

    fn trim_front(&mut self, count: usize) -> Option<TrimEvent> {
        if count == 0 {
            return None;
        }
        let mut trimmed = 0usize;
        for _ in 0..count {
            if self.rows.pop_front().is_some() {
                trimmed += 1;
            } else {
                break;
            }
        }
        if trimmed == 0 {
            return None;
        }
        let start = self.base;
        self.base = self
            .rows
            .front()
            .map(|entry| entry.absolute)
            .unwrap_or(self.next_row_id);
        Some(TrimEvent {
            start,
            count: trimmed,
        })
    }
}

/// Terminal-specific grid cache that stores packed terminal cells alongside a
/// deduplicated style table.
pub struct TerminalGrid {
    inner: RwLock<GridInner>,
    pub style_table: Arc<StyleTable>,
    default_cell: PackedCell,
    default_seq: Seq,
    history_limit: usize,
    trim_events: Mutex<Vec<TrimEvent>>,
    viewport_rows: AtomicUsize,
    viewport_cols: AtomicUsize,
}

/// Snapshot wrapper returned when reading a cell from the terminal grid.
#[derive(Clone, Copy, Debug)]
pub struct TerminalCellSnapshot {
    pub cell: PackedCell,
    pub seq: Seq,
}

impl From<CellSnapshot> for TerminalCellSnapshot {
    fn from(snapshot: CellSnapshot) -> Self {
        TerminalCellSnapshot {
            cell: PackedCell::from(snapshot.payload),
            seq: snapshot.seq,
        }
    }
}

impl TerminalCellSnapshot {
    /// Convert the packed payload back into a heavy [`Cell`](HeavyCell).
    pub fn unpack(&self, style_table: &StyleTable) -> HeavyCell {
        unpack_to_heavy(self.cell, style_table)
    }
}

impl TerminalGrid {
    pub fn new(rows: usize, cols: usize) -> Self {
        let style_table = Arc::new(StyleTable::new());
        let default_cell = pack_cell(' ', StyleId::DEFAULT);
        let default_seq = 0;
        let inner = GridInner::new(rows, cols, default_cell, default_seq);
        let visible_rows = rows.max(1);
        let visible_cols = cols.max(1);

        Self {
            inner: RwLock::new(inner),
            style_table,
            default_cell,
            default_seq,
            history_limit: DEFAULT_HISTORY_LIMIT.max(rows.max(1)),
            trim_events: Mutex::new(Vec::new()),
            viewport_rows: AtomicUsize::new(visible_rows),
            viewport_cols: AtomicUsize::new(visible_cols),
        }
    }

    pub fn with_history_limit(rows: usize, cols: usize, history_limit: usize) -> Self {
        let mut grid = Self::new(rows, cols);
        grid.history_limit = history_limit.max(rows.max(1));
        grid
    }

    pub fn viewport_size(&self) -> (usize, usize) {
        (
            self.viewport_rows.load(Ordering::Relaxed),
            self.viewport_cols.load(Ordering::Relaxed),
        )
    }

    pub fn set_viewport_size(&self, rows: usize, cols: usize) {
        self.viewport_rows.store(rows.max(1), Ordering::Relaxed);
        self.viewport_cols.store(cols.max(1), Ordering::Relaxed);
    }

    pub fn clear_viewport(&self) {
        let mut inner = self.inner.write().unwrap();
        let old_base = inner.base;
        let trimmed_count = inner.rows.len();
        let cols = inner.cols;
        let viewport_rows = self.viewport_rows.load(Ordering::Relaxed).max(1);
        inner.rows.clear();
        for _ in 0..viewport_rows {
            let absolute = inner.next_row_id;
            inner.rows.push_back(RowEntry::new(
                absolute,
                cols,
                self.default_cell,
                self.default_seq,
            ));
            inner.next_row_id = inner.next_row_id.saturating_add(1);
        }
        inner.base = inner
            .rows
            .front()
            .map(|entry| entry.absolute)
            .unwrap_or(inner.next_row_id);
        if trimmed_count > 0 {
            self.trim_events.lock().unwrap().push(TrimEvent {
                start: old_base,
                count: trimmed_count,
            });
        }
    }

    pub fn ensure_style_id(&self, style: Style) -> StyleId {
        self.style_table.ensure_id(style)
    }

    pub fn pack_char_with_style(ch: char, style_id: StyleId) -> PackedCell {
        pack_cell(ch, style_id)
    }

    fn guard_trim_events(&self, event: Option<TrimEvent>) {
        if let Some(event) = event {
            self.trim_events.lock().unwrap().push(event);
        }
    }

    pub fn drain_trim_events(&self) -> Vec<TrimEvent> {
        let mut guard = self.trim_events.lock().unwrap();
        guard.drain(..).collect()
    }

    pub fn row_offset(&self) -> u64 {
        self.inner.read().unwrap().base
    }

    pub fn set_row_offset(&self, base: u64) {
        let mut inner = self.inner.write().unwrap();
        let old_base = inner.base;
        inner.base = base;
        let mut absolute = base;
        for entry in inner.rows.iter_mut() {
            entry.absolute = absolute;
            absolute = absolute.saturating_add(1);
        }
        inner.next_row_id = absolute;
        if base > old_base {
            let diff = base - old_base;
            if let Ok(count) = usize::try_from(diff) {
                self.guard_trim_events(Some(TrimEvent {
                    start: old_base,
                    count,
                }));
            } else {
                self.guard_trim_events(Some(TrimEvent {
                    start: old_base,
                    count: usize::MAX,
                }));
            }
        }
        tracing::trace!(
            target = "server::grid",
            row_offset = base,
            rows = inner.rows.len()
        );
    }

    pub fn first_row_id(&self) -> Option<u64> {
        self.inner
            .read()
            .unwrap()
            .rows
            .front()
            .map(|entry| entry.absolute)
    }

    pub fn last_row_id(&self) -> Option<u64> {
        self.inner
            .read()
            .unwrap()
            .rows
            .back()
            .map(|entry| entry.absolute)
    }

    pub fn row_id_at(&self, index: usize) -> Option<u64> {
        self.inner
            .read()
            .unwrap()
            .rows
            .get(index)
            .map(|entry| entry.absolute)
    }

    pub fn index_of_row(&self, absolute: u64) -> Option<usize> {
        let inner = self.inner.read().unwrap();
        if absolute < inner.base {
            return None;
        }
        let offset = (absolute - inner.base) as usize;
        if offset < inner.rows.len() {
            Some(offset)
        } else {
            None
        }
    }

    pub fn next_row_id(&self) -> u64 {
        self.inner.read().unwrap().next_row_id
    }

    pub fn history_limit(&self) -> usize {
        self.history_limit
    }

    fn enforce_history_limit(&self, inner: &mut GridInner) {
        if inner.len() <= self.history_limit {
            return;
        }
        let overflow = inner.len() - self.history_limit;
        let event = inner.trim_front(overflow);
        self.guard_trim_events(event);
    }

    fn ensure_row_and_col(
        &self,
        row: usize,
        col: usize,
    ) -> Result<(std::sync::RwLockWriteGuard<'_, GridInner>, usize), WriteError> {
        let mut inner = self.inner.write().unwrap();
        inner.ensure_cols(col + 1, self.default_cell, self.default_seq);
        let absolute = row as u64;
        let index = inner.ensure_row(absolute, self.default_cell, self.default_seq)?;
        Ok((inner, index))
    }

    pub fn write_packed_cell_if_newer(
        &self,
        row: usize,
        col: usize,
        seq: Seq,
        cell: PackedCell,
    ) -> Result<WriteOutcome, WriteError> {
        let (mut inner, index) = self.ensure_row_and_col(row, col)?;
        let outcome = {
            let row_entry = inner.rows.get_mut(index).unwrap();
            row_entry.write_cell_if_newer(col, seq, cell)?
        };
        self.enforce_history_limit(&mut inner);
        Ok(outcome)
    }

    pub fn fill_rect_with_cell_if_newer(
        &self,
        row0: usize,
        col0: usize,
        row1: usize,
        col1: usize,
        seq: Seq,
        cell: PackedCell,
    ) -> Result<(usize, usize), WriteError> {
        if row0 > row1 || col0 > col1 {
            return Ok((0, 0));
        }
        let mut inner = self.inner.write().unwrap();
        inner.ensure_cols(col1, self.default_cell, self.default_seq);
        let mut written = 0usize;
        let mut skipped = 0usize;
        for row in row0..row1 {
            let absolute = row as u64;
            let index = inner.ensure_row(absolute, self.default_cell, self.default_seq)?;
            if let Some(entry) = inner.rows.get_mut(index) {
                let (w, s) = entry.fill_rect_if_newer(col0, col1, seq, cell)?;
                written += w;
                skipped += s;
            }
        }
        self.enforce_history_limit(&mut inner);
        Ok((written, skipped))
    }

    pub fn get_cell_relaxed(&self, row: usize, col: usize) -> Option<TerminalCellSnapshot> {
        let inner = self.inner.read().unwrap();
        let entry = inner.rows.get(row)?;
        entry.cell_snapshot(col).map(TerminalCellSnapshot::from)
    }

    pub fn snapshot_row_into(&self, row: usize, out: &mut [u64]) -> Result<(), WriteError> {
        let inner = self.inner.read().unwrap();
        if let Some(entry) = inner.rows.get(row) {
            entry.snapshot_row(out)
        } else {
            Err(WriteError::CoordOutOfBounds)
        }
    }

    pub fn rows(&self) -> usize {
        self.inner.read().unwrap().len()
    }

    pub fn cols(&self) -> usize {
        self.inner.read().unwrap().cols()
    }

    pub fn freeze_row(&mut self, _row: usize) -> Result<(), WriteError> {
        Ok(())
    }

    pub fn thaw_row(&mut self, _row: usize, _seq: Seq) -> Result<(), WriteError> {
        Ok(())
    }
}

impl GridCache for TerminalGrid {
    fn dims(&self) -> (usize, usize) {
        let inner = self.inner.read().unwrap();
        (inner.len(), inner.cols())
    }

    fn write_cell_if_newer(
        &self,
        row: usize,
        col: usize,
        seq: Seq,
        payload: u64,
    ) -> Result<WriteOutcome, WriteError> {
        self.write_packed_cell_if_newer(row, col, seq, PackedCell::from(payload))
    }

    fn fill_rect_if_newer(
        &self,
        row0: usize,
        col0: usize,
        row1: usize,
        col1: usize,
        seq: Seq,
        payload: u64,
    ) -> Result<(usize, usize), WriteError> {
        self.fill_rect_with_cell_if_newer(row0, col0, row1, col1, seq, PackedCell::from(payload))
    }

    fn snapshot_row_into(&self, row: usize, out: &mut [u64]) -> Result<(), WriteError> {
        TerminalGrid::snapshot_row_into(self, row, out)
    }

    fn get_cell_relaxed(&self, row: usize, col: usize) -> Option<CellSnapshot> {
        let inner = self.inner.read().unwrap();
        inner
            .rows
            .get(row)
            .and_then(|entry| entry.cell_snapshot(col))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::terminal::pack_from_heavy;
    use crate::model::terminal::cell::{Cell, CellAttributes, Color};
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test_timeout::timeout]
    fn writes_roundtrip_through_style_table() {
        let grid = TerminalGrid::new(2, 3);
        let style_table = grid.style_table.as_ref();

        let rows = vec![
            vec![
                Cell {
                    char: 'H',
                    fg_color: Color::Rgb(255, 0, 0),
                    bg_color: Color::Default,
                    attributes: CellAttributes {
                        bold: true,
                        ..CellAttributes::default()
                    },
                },
                Cell {
                    char: 'i',
                    fg_color: Color::Indexed(4),
                    bg_color: Color::Rgb(0, 0, 0),
                    attributes: CellAttributes {
                        underline: true,
                        ..CellAttributes::default()
                    },
                },
                Cell {
                    char: '!',
                    fg_color: Color::Default,
                    bg_color: Color::Default,
                    attributes: CellAttributes::default(),
                },
            ],
            vec![
                Cell {
                    char: '🌊',
                    fg_color: Color::Rgb(0, 128, 255),
                    bg_color: Color::Rgb(10, 10, 10),
                    attributes: CellAttributes {
                        italic: true,
                        ..CellAttributes::default()
                    },
                },
                Cell {
                    char: 'B',
                    fg_color: Color::Rgb(255, 255, 255),
                    bg_color: Color::Rgb(0, 0, 0),
                    attributes: CellAttributes {
                        reverse: true,
                        ..CellAttributes::default()
                    },
                },
                Cell {
                    char: 'E',
                    fg_color: Color::Rgb(255, 255, 0),
                    bg_color: Color::Default,
                    attributes: CellAttributes {
                        underline: true,
                        ..CellAttributes::default()
                    },
                },
            ],
        ];

        for (row_idx, row) in rows.iter().enumerate() {
            for (col_idx, heavy) in row.iter().enumerate() {
                let packed = pack_from_heavy(heavy, style_table);
                grid.write_packed_cell_if_newer(row_idx, col_idx, 5, packed)
                    .expect("write cell");
            }
        }

        for (row_idx, row) in rows.iter().enumerate() {
            for (col_idx, heavy) in row.iter().enumerate() {
                let snapshot = grid
                    .get_cell_relaxed(row_idx, col_idx)
                    .expect("cell exists");
                let unpacked = snapshot.unpack(style_table);
                assert_eq!(unpacked.char, heavy.char);
                assert_eq!(unpacked.fg_color, heavy.fg_color);
                assert_eq!(unpacked.bg_color, heavy.bg_color);
                assert_eq!(unpacked.attributes, heavy.attributes);
            }
        }
    }

    #[test_timeout::timeout]
    fn concurrent_writes_prefer_latest_seq() {
        let grid = Arc::new(TerminalGrid::new(1, 1));
        let iterations = 1000usize;
        let writers = 4usize;
        let barrier = Arc::new(Barrier::new(writers));

        let mut handles = Vec::new();
        for worker in 0..writers {
            let grid = grid.clone();
            let barrier = barrier.clone();
            handles.push(thread::spawn(move || {
                barrier.wait();
                for i in 0..iterations {
                    let seq = (worker * iterations + i) as u64;
                    let cell = TerminalGrid::pack_char_with_style('a', StyleId::DEFAULT);
                    grid.write_packed_cell_if_newer(0, 0, seq, cell).unwrap();
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let snapshot = grid.get_cell_relaxed(0, 0).unwrap();
        assert_eq!(snapshot.seq, (writers * iterations - 1) as u64);
    }

    #[test_timeout::timeout]
    fn history_trims_emit_events() {
        let grid = TerminalGrid::with_history_limit(2, 1, 3);
        let style_id = grid.ensure_style_id(Style::default());
        let packed = TerminalGrid::pack_char_with_style('x', style_id);

        for row in 0..10 {
            grid.write_packed_cell_if_newer(row, 0, row as u64 + 1, packed)
                .expect("write row");
        }

        let events = grid.drain_trim_events();
        assert!(!events.is_empty());
        let total_trimmed: usize = events.iter().map(|event| event.count).sum();
        assert!(total_trimmed >= 7);
        assert!(grid.rows() <= 3);
    }
}

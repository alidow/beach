use std::sync::Arc;

use super::packed::{PackedCell, Style, StyleId, StyleTable, pack_cell, unpack_to_heavy};
use crate::cache::grid::AtomicGrid;
use crate::cache::{CellSnapshot, GridCache, Seq, WriteError, WriteOutcome};
use crate::model::terminal::cell::Cell as HeavyCell;

/// Terminal-specific grid cache that stores packed terminal cells alongside a
/// deduplicated style table. Writing typically follows three steps:
/// 1. Obtain or create a [`StyleId`] via [`TerminalGrid::ensure_style_id`]
/// 2. Pack a cell with [`pack_cell`] (or use [`TerminalGrid::pack_char_with_style`])
/// 3. Submit it with [`TerminalGrid::write_packed_cell_if_newer`]
///
/// ```rust
/// # use beach_human::cache::terminal::{TerminalGrid, Style, StyleId};
/// # use beach_human::cache::{WriteOutcome, GridCache};
/// let grid = TerminalGrid::new(24, 80);
/// let style = Style::default();
/// let style_id = grid.ensure_style_id(style);
/// let packed = TerminalGrid::pack_char_with_style('A', style_id);
/// assert_eq!(
///     grid.write_packed_cell_if_newer(0, 0, 5, packed).unwrap(),
///     WriteOutcome::Written,
/// );
/// let snapshot = grid.get_cell_relaxed(0, 0).unwrap();
/// assert_eq!(snapshot.seq, 5);
/// ```
pub struct TerminalGrid {
    pub grid: AtomicGrid,
    pub style_table: Arc<StyleTable>,
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
        let default_payload = pack_cell(' ', StyleId::DEFAULT).into_raw();
        let grid = AtomicGrid::new(rows, cols, default_payload, 0);
        Self { grid, style_table }
    }

    pub fn ensure_style_id(&self, style: Style) -> StyleId {
        self.style_table.ensure_id(style)
    }

    pub fn pack_char_with_style(ch: char, style_id: StyleId) -> PackedCell {
        pack_cell(ch, style_id)
    }

    pub fn write_packed_cell_if_newer(
        &self,
        row: usize,
        col: usize,
        seq: Seq,
        cell: PackedCell,
    ) -> Result<WriteOutcome, WriteError> {
        self.grid
            .write_cell_if_newer(row, col, seq, cell.into_raw())
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
        self.grid
            .fill_rect_if_newer(row0, col0, row1, col1, seq, cell.into_raw())
    }

    pub fn freeze_row(&mut self, row: usize) -> Result<(), WriteError> {
        self.grid.freeze_row(row)
    }
    pub fn thaw_row(&mut self, row: usize, seq: Seq) -> Result<(), WriteError> {
        self.grid.thaw_row(row, seq)
    }

    pub fn get_cell_relaxed(&self, row: usize, col: usize) -> Option<TerminalCellSnapshot> {
        self.grid
            .get_cell_relaxed(row, col)
            .map(TerminalCellSnapshot::from)
    }
}

impl GridCache for TerminalGrid {
    fn dims(&self) -> (usize, usize) {
        self.grid.dims()
    }

    fn write_cell_if_newer(
        &self,
        row: usize,
        col: usize,
        seq: Seq,
        payload: u64,
    ) -> Result<WriteOutcome, WriteError> {
        self.grid.write_cell_if_newer(row, col, seq, payload)
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
        self.grid
            .fill_rect_if_newer(row0, col0, row1, col1, seq, payload)
    }

    fn snapshot_row_into(&self, row: usize, out: &mut [u64]) -> Result<(), WriteError> {
        self.grid.snapshot_row_into(row, out)
    }

    fn get_cell_relaxed(&self, row: usize, col: usize) -> Option<CellSnapshot> {
        self.grid.get_cell_relaxed(row, col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::terminal::pack_from_heavy;
    use crate::model::terminal::cell::{Cell, CellAttributes, Color};
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;
    use std::time::Duration;

    #[test]
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
                    bg_color: Color::Indexed(2),
                    attributes: CellAttributes {
                        reverse: true,
                        ..CellAttributes::default()
                    },
                },
                Cell::default(),
            ],
        ];

        for (row_idx, row) in rows.iter().enumerate() {
            for (col_idx, cell) in row.iter().enumerate() {
                let packed = pack_from_heavy(cell, style_table);
                let seq = (row_idx * 10 + col_idx + 1) as Seq;
                let outcome = grid
                    .write_packed_cell_if_newer(row_idx, col_idx, seq, packed)
                    .expect("write succeeds");
                assert_eq!(outcome, WriteOutcome::Written);
            }
        }

        for (row_idx, row) in rows.iter().enumerate() {
            for (col_idx, expected) in row.iter().enumerate() {
                let seq = (row_idx * 10 + col_idx + 1) as Seq;
                let snapshot = grid
                    .get_cell_relaxed(row_idx, col_idx)
                    .expect("cell present");
                assert_eq!(snapshot.seq, seq);
                let unpacked = snapshot.unpack(style_table);
                assert_eq!(&unpacked, expected);
            }
        }

        assert!(grid.style_table.len() >= 3);
    }

    #[test]
    fn concurrent_writes_and_reads_preserve_latest_seq() {
        let grid = Arc::new(TerminalGrid::new(1, 1));
        let style_id = grid.ensure_style_id(Style::default());
        let barrier = Arc::new(Barrier::new(3));
        let outcomes = Arc::new(Mutex::new(Vec::new()));
        let reader_observations = Arc::new(Mutex::new(Vec::new()));

        // Reader thread snapshots the cell repeatedly while writers race.
        let reader_handle = {
            let grid = Arc::clone(&grid);
            let barrier = Arc::clone(&barrier);
            let observations = Arc::clone(&reader_observations);
            thread::spawn(move || {
                barrier.wait();
                for _ in 0..8 {
                    if let Some(snapshot) = grid.get_cell_relaxed(0, 0) {
                        observations.lock().unwrap().push(snapshot.seq);
                    }
                    thread::yield_now();
                }
            })
        };

        let spawn_writer = |seq: Seq, ch: char, pause: Option<Duration>| {
            let grid = Arc::clone(&grid);
            let barrier = Arc::clone(&barrier);
            let outcomes = Arc::clone(&outcomes);
            thread::spawn(move || {
                barrier.wait();
                if let Some(delay) = pause {
                    thread::sleep(delay);
                }
                let packed = TerminalGrid::pack_char_with_style(ch, style_id);
                let outcome = grid
                    .write_packed_cell_if_newer(0, 0, seq, packed)
                    .expect("write succeeds");
                outcomes.lock().unwrap().push((seq, outcome));
            })
        };

        let writer_high = spawn_writer(2, 'Y', None);
        let writer_low = spawn_writer(1, 'X', Some(Duration::from_millis(10)));

        writer_high.join().unwrap();
        writer_low.join().unwrap();
        reader_handle.join().unwrap();

        let snapshot = grid.get_cell_relaxed(0, 0).expect("cell present");
        assert_eq!(snapshot.seq, 2);
        let unpacked = snapshot.unpack(grid.style_table.as_ref());
        assert_eq!(unpacked.char, 'Y');

        let outcomes = outcomes.lock().unwrap();
        let high_outcome = outcomes
            .iter()
            .find(|(seq, _)| *seq == 2)
            .map(|(_, outcome)| *outcome)
            .expect("high seq outcome present");
        assert_eq!(high_outcome, WriteOutcome::Written);

        let low_outcome = outcomes
            .iter()
            .find(|(seq, _)| *seq == 1)
            .map(|(_, outcome)| *outcome)
            .expect("low seq outcome present");
        assert_eq!(low_outcome, WriteOutcome::SkippedOlder);

        let reader_observations = reader_observations.lock().unwrap();
        assert!(reader_observations.iter().any(|&seq| seq == 2));
        assert!(reader_observations.iter().all(|&seq| seq <= 2));

        let fallback = grid
            .write_packed_cell_if_newer(0, 0, 1, TerminalGrid::pack_char_with_style('Z', style_id))
            .expect("write succeeds");
        assert_eq!(fallback, WriteOutcome::SkippedOlder);
    }

    #[test]
    fn concurrent_rect_writes_resolve_latest_per_cell() {
        #[derive(Clone)]
        struct RectOp {
            seq: Seq,
            ch: char,
            row0: usize,
            col0: usize,
            row1: usize,
            col1: usize,
            delay_ms: u64,
        }

        let rows = 3usize;
        let cols = 4usize;
        let grid = Arc::new(TerminalGrid::new(rows, cols));
        let style_id = grid.ensure_style_id(Style::default());

        let operations = vec![
            RectOp {
                seq: 5,
                ch: 'A',
                row0: 0,
                col0: 0,
                row1: 3,
                col1: 4,
                delay_ms: 5,
            },
            RectOp {
                seq: 7,
                ch: 'B',
                row0: 1,
                col0: 1,
                row1: 3,
                col1: 3,
                delay_ms: 0,
            },
            RectOp {
                seq: 6,
                ch: 'C',
                row0: 0,
                col0: 2,
                row1: 2,
                col1: 4,
                delay_ms: 2,
            },
            RectOp {
                seq: 9,
                ch: 'D',
                row0: 2,
                col0: 0,
                row1: 3,
                col1: 4,
                delay_ms: 1,
            },
        ];

        let mut expected = vec![vec![(0u64, ' '); cols]; rows];
        for op in &operations {
            for row in op.row0..op.row1 {
                for col in op.col0..op.col1 {
                    if op.seq >= expected[row][col].0 {
                        expected[row][col] = (op.seq, op.ch);
                    }
                }
            }
        }

        let barrier = Arc::new(Barrier::new(operations.len() + 1));
        let results: Arc<Mutex<Vec<(Seq, usize, usize, usize)>>> = Arc::new(Mutex::new(Vec::new()));
        let mut handles = Vec::new();

        for op in operations.iter().cloned() {
            let grid = Arc::clone(&grid);
            let barrier = Arc::clone(&barrier);
            let results = Arc::clone(&results);
            handles.push(thread::spawn(move || {
                barrier.wait();
                if op.delay_ms > 0 {
                    thread::sleep(Duration::from_millis(op.delay_ms));
                }
                let packed = TerminalGrid::pack_char_with_style(op.ch, style_id);
                let (written, skipped) = grid
                    .fill_rect_with_cell_if_newer(
                        op.row0, op.col0, op.row1, op.col1, op.seq, packed,
                    )
                    .expect("rect write succeeds");
                let area = (op.row1 - op.row0) * (op.col1 - op.col0);
                results
                    .lock()
                    .unwrap()
                    .push((op.seq, written, skipped, area));
            }));
        }

        barrier.wait();
        for handle in handles {
            handle.join().unwrap();
        }

        let results = results.lock().unwrap();
        assert_eq!(results.len(), operations.len());
        for &(seq, written, skipped, area) in results.iter() {
            assert_eq!(written + skipped, area, "seq {seq} area mismatch");
            if seq == 9 {
                assert!(written > 0, "highest seq should write at least once");
            }
        }

        for row in 0..rows {
            for col in 0..cols {
                let snapshot = grid.get_cell_relaxed(row, col).expect("cell present");
                let (expected_seq, expected_char) = expected[row][col];
                assert_eq!(snapshot.seq, expected_seq, "row {row} col {col}");
                let unpacked = snapshot.unpack(grid.style_table.as_ref());
                assert_eq!(unpacked.char, expected_char, "row {row} col {col}");
            }
        }
    }
}

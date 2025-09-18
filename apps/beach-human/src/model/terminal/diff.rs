use std::ops::Range;

use crate::cache::Seq;
use crate::cache::terminal::PackedCell;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellWrite {
    pub row: usize,
    pub col: usize,
    pub seq: Seq,
    pub cell: PackedCell,
}

impl CellWrite {
    pub fn new(row: usize, col: usize, seq: Seq, cell: PackedCell) -> Self {
        Self {
            row,
            col,
            seq,
            cell,
        }
    }

    pub fn seq(&self) -> Seq {
        self.seq
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RectFill {
    pub rows: Range<usize>,
    pub cols: Range<usize>,
    pub seq: Seq,
    pub cell: PackedCell,
}

impl RectFill {
    pub fn new(rows: Range<usize>, cols: Range<usize>, seq: Seq, cell: PackedCell) -> Self {
        Self {
            rows,
            cols,
            seq,
            cell,
        }
    }

    pub fn area(&self) -> usize {
        self.rows.len() * self.cols.len()
    }

    pub fn seq(&self) -> Seq {
        self.seq
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowSnapshot {
    pub row: usize,
    pub seq: Seq,
    pub cells: Vec<PackedCell>,
}

impl RowSnapshot {
    pub fn new(row: usize, seq: Seq, cells: Vec<PackedCell>) -> Self {
        Self { row, seq, cells }
    }

    pub fn width(&self) -> usize {
        self.cells.len()
    }

    pub fn seq(&self) -> Seq {
        self.seq
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CacheUpdate {
    Cell(CellWrite),
    Rect(RectFill),
    Row(RowSnapshot),
}

impl CacheUpdate {
    pub fn seq(&self) -> Seq {
        match self {
            CacheUpdate::Cell(cell) => cell.seq,
            CacheUpdate::Rect(rect) => rect.seq,
            CacheUpdate::Row(row) => row.seq,
        }
    }
}

use std::ops::Range;

use crate::cache::Seq;
use crate::cache::terminal::{PackedCell, Style, StyleId};
use crate::model::terminal::CursorState;

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
pub struct HistoryTrim {
    pub start: usize,
    pub count: usize,
}

impl HistoryTrim {
    pub fn new(start: usize, count: usize) -> Self {
        Self { start, count }
    }

    pub fn end(&self) -> usize {
        self.start.saturating_add(self.count)
    }

    pub fn seq(&self) -> Seq {
        self.end() as Seq
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyleDefinition {
    pub id: StyleId,
    pub seq: Seq,
    pub style: Style,
}

impl StyleDefinition {
    pub fn new(id: StyleId, seq: Seq, style: Style) -> Self {
        Self { id, seq, style }
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
    Trim(HistoryTrim),
    Style(StyleDefinition),
    Cursor(CursorState),
}

impl CacheUpdate {
    pub fn seq(&self) -> Seq {
        match self {
            CacheUpdate::Cell(cell) => cell.seq,
            CacheUpdate::Rect(rect) => rect.seq,
            CacheUpdate::Row(row) => row.seq,
            CacheUpdate::Trim(trim) => trim.seq(),
            CacheUpdate::Style(style) => style.seq(),
            CacheUpdate::Cursor(cursor) => cursor.seq,
        }
    }
}

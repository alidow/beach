use crate::cache::Seq;

use super::cell::Cell;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalLine {
    pub row: usize,
    pub seq: Seq,
    pub cells: Vec<Cell>,
}

impl TerminalLine {
    pub fn new(row: usize, seq: Seq, cells: Vec<Cell>) -> Self {
        Self { row, seq, cells }
    }

    pub fn width(&self) -> usize {
        self.cells.len()
    }

    pub fn is_blank(&self) -> bool {
        self.cells.iter().all(Cell::is_blank)
    }
}

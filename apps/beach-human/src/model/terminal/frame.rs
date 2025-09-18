use crate::cache::Seq;

use super::line::TerminalLine;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalFrame {
    pub rows: usize,
    pub cols: usize,
    pub lines: Vec<TerminalLine>,
    pub watermark: Seq,
}

impl TerminalFrame {
    pub fn new(rows: usize, cols: usize, lines: Vec<TerminalLine>, watermark: Seq) -> Self {
        Self {
            rows,
            cols,
            lines,
            watermark,
        }
    }

    pub fn line(&self, row: usize) -> Option<&TerminalLine> {
        self.lines.iter().find(|l| l.row == row)
    }
}

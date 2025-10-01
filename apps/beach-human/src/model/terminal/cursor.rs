use crate::cache::Seq;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorPosition {
    pub row: usize,
    pub col: usize,
    pub seq: Seq,
}

impl CursorPosition {
    pub fn new(row: usize, col: usize, seq: Seq) -> Self {
        Self { row, col, seq }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorState {
    pub row: usize,
    pub col: usize,
    pub seq: Seq,
    pub visible: bool,
    pub blink: bool,
}

impl CursorState {
    pub fn new(row: usize, col: usize, seq: Seq, visible: bool, blink: bool) -> Self {
        Self {
            row,
            col,
            seq,
            visible,
            blink,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport {
    pub rows: usize,
    pub cols: usize,
    pub scroll_offset: isize,
}

impl Viewport {
    pub fn new(rows: usize, cols: usize, scroll_offset: isize) -> Self {
        Self {
            rows,
            cols,
            scroll_offset,
        }
    }

    pub fn contains(&self, row: isize) -> bool {
        let top = self.scroll_offset;
        let bottom = top + self.rows as isize;
        row >= top && row < bottom
    }
}

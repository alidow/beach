use crate::cache::terminal::{pack_cell, PackedCell, Style, StyleId, TerminalGrid};
use crate::cache::GridCache;
use crate::cache::Seq;
use crate::model::terminal::diff::{CacheUpdate, CellWrite, RowSnapshot};
use std::borrow::Cow;

pub type EmulatorResult = Vec<CacheUpdate>;

pub trait TerminalEmulator {
    fn handle_output(&mut self, chunk: &[u8], grid: &TerminalGrid) -> EmulatorResult;
    fn flush(&mut self, _grid: &TerminalGrid) -> EmulatorResult {
        Vec::new()
    }
    fn resize(&mut self, rows: usize, cols: usize) {
        let _ = (rows, cols);
    }
}

pub struct SimpleTerminalEmulator {
    rows: usize,
    cols: usize,
    row: usize,
    col: usize,
    seq: Seq,
    default_style: StyleId,
    line_buffer: Vec<PackedCell>,
}

impl SimpleTerminalEmulator {
    pub fn new(grid: &TerminalGrid) -> Self {
        let (rows, cols) = grid.dims();
        let rows = rows.max(1);
        let cols = cols.max(1);
        let default_style = grid.ensure_style_id(Style::default());
        Self {
            rows,
            cols,
            row: 0,
            col: 0,
            seq: 0,
            default_style,
            line_buffer: Vec::with_capacity(cols),
        }
    }

    fn advance_row(&mut self) {
        self.row = (self.row + 1).min(self.rows.saturating_sub(1));
        self.col = 0;
        self.line_buffer.clear();
    }

    fn push_char(&mut self, ch: char) -> CacheUpdate {
        if self.col >= self.cols {
            self.advance_row();
        }
        self.seq += 1;
        let cell = pack_cell(ch, self.default_style);
        let update = CacheUpdate::Cell(CellWrite::new(self.row, self.col, self.seq, cell));
        self.col += 1;
        update
    }
}

impl TerminalEmulator for SimpleTerminalEmulator {
    fn handle_output(&mut self, chunk: &[u8], _grid: &TerminalGrid) -> EmulatorResult {
        if chunk.is_empty() {
            return Vec::new();
        }

        let mut updates = Vec::new();
        let text: Cow<'_, str> = match std::str::from_utf8(chunk) {
            Ok(s) => Cow::Borrowed(s),
            Err(_) => Cow::Owned(String::from_utf8_lossy(chunk).into_owned()),
        };

        for ch in text.chars() {
            match ch {
                '\n' => {
                    if !self.line_buffer.is_empty() {
                        self.seq += 1;
                        updates.push(CacheUpdate::Row(RowSnapshot::new(
                            self.row,
                            self.seq,
                            self.line_buffer.clone(),
                        )));
                        self.line_buffer.clear();
                    }
                    self.advance_row();
                }
                '\r' => {
                    self.col = 0;
                    self.line_buffer.clear();
                }
                '\t' => {
                    let next_tab = ((self.col / 4) + 1) * 4;
                    while self.col < self.cols && self.col < next_tab {
                        updates.push(self.push_char(' '));
                    }
                }
                '\u{0008}' => {
                    if self.col > 0 {
                        self.col -= 1;
                    }
                }
                other => {
                    let update = self.push_char(other);
                    if let CacheUpdate::Cell(cell) = &update {
                        self.line_buffer.push(cell.cell);
                    }
                    updates.push(update);
                }
            }
        }

        updates
    }

    fn flush(&mut self, _grid: &TerminalGrid) -> EmulatorResult {
        if self.line_buffer.is_empty() {
            Vec::new()
        } else {
            self.seq += 1;
            let snapshot =
                RowSnapshot::new(self.row, self.seq, std::mem::take(&mut self.line_buffer));
            vec![CacheUpdate::Row(snapshot)]
        }
    }

    fn resize(&mut self, rows: usize, cols: usize) {
        self.rows = rows.max(1);
        self.cols = cols.max(1);
        if self.row >= self.rows {
            self.row = self.rows.saturating_sub(1);
        }
        if self.col >= self.cols {
            self.col = self.cols.saturating_sub(1);
        }
        self.line_buffer.clear();
        self.line_buffer.reserve(self.cols);
    }
}

pub struct AlacrittyEmulator {
    inner: SimpleTerminalEmulator,
}

impl AlacrittyEmulator {
    pub fn new(grid: &TerminalGrid) -> Self {
        Self {
            inner: SimpleTerminalEmulator::new(grid),
        }
    }
}

impl TerminalEmulator for AlacrittyEmulator {
    fn handle_output(&mut self, chunk: &[u8], grid: &TerminalGrid) -> EmulatorResult {
        self.inner.handle_output(chunk, grid)
    }

    fn flush(&mut self, grid: &TerminalGrid) -> EmulatorResult {
        self.inner.flush(grid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::terminal::unpack_cell;
    use crate::cache::terminal::TerminalGrid;

    #[test]
    fn ascii_output_produces_cell_updates() {
        let grid = TerminalGrid::new(4, 10);
        let mut emulator = SimpleTerminalEmulator::new(&grid);
        let updates = emulator.handle_output(b"hi", &grid);
        assert_eq!(updates.len(), 2);
        match &updates[0] {
            CacheUpdate::Cell(cell) => {
                let (ch, _) = unpack_cell(cell.cell);
                assert_eq!(ch, 'h');
            }
            _ => panic!("expected cell update"),
        }
    }
}

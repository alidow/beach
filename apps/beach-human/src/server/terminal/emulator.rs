use crate::cache::GridCache;
use crate::cache::Seq;
use crate::cache::terminal::{PackedCell, Style, StyleId, TerminalGrid, pack_cell};
use crate::model::terminal::diff::{CacheUpdate, CellWrite, RowSnapshot};
use std::borrow::Cow;

pub type EmulatorResult = Vec<CacheUpdate>;

pub trait TerminalEmulator: Send {
    fn handle_output(&mut self, chunk: &[u8], grid: &TerminalGrid) -> EmulatorResult;
    fn flush(&mut self, grid: &TerminalGrid) -> EmulatorResult {
        let _ = grid;
        Vec::new()
    }
    fn resize(&mut self, rows: usize, cols: usize);
}

pub struct SimpleTerminalEmulator {
    viewport_rows: usize,
    viewport_cols: usize,
    absolute_row: usize,
    col: usize,
    seq: Seq,
    default_style: StyleId,
    line_buffer: Vec<PackedCell>,
}

unsafe impl Send for SimpleTerminalEmulator {}

impl SimpleTerminalEmulator {
    pub fn new(grid: &TerminalGrid) -> Self {
        let (rows, cols) = grid.dims();
        let viewport_rows = rows.max(1);
        let viewport_cols = cols.max(1);
        let default_style = grid.ensure_style_id(Style::default());
        Self {
            viewport_rows,
            viewport_cols,
            absolute_row: 0,
            col: 0,
            seq: 0,
            default_style,
            line_buffer: Vec::with_capacity(viewport_cols),
        }
    }

    fn advance_row(&mut self) {
        self.absolute_row = self.absolute_row.saturating_add(1);
        self.col = 0;
        self.line_buffer.clear();
    }

    fn push_char(&mut self, ch: char) -> CacheUpdate {
        if self.col >= self.viewport_cols {
            self.advance_row();
        }
        self.seq = self.seq.saturating_add(1);
        let cell = pack_cell(ch, self.default_style);
        let update = CacheUpdate::Cell(CellWrite::new(self.absolute_row, self.col, self.seq, cell));
        self.col = self.col.saturating_add(1);
        update
    }

    fn emit_line_snapshot(&mut self) -> Option<CacheUpdate> {
        if self.line_buffer.is_empty() {
            None
        } else {
            self.seq = self.seq.saturating_add(1);
            let snapshot = RowSnapshot::new(self.absolute_row, self.seq, self.line_buffer.clone());
            self.line_buffer.clear();
            Some(CacheUpdate::Row(snapshot))
        }
    }

    fn process_chunk(&mut self, chunk: &[u8]) -> EmulatorResult {
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
                    if let Some(snapshot) = self.emit_line_snapshot() {
                        updates.push(snapshot);
                    }
                    self.advance_row();
                }
                '\r' => {
                    self.col = 0;
                    self.line_buffer.clear();
                }
                '\t' => {
                    let tab_width = 4usize;
                    let next_tab_stop = ((self.col / tab_width) + 1) * tab_width;
                    while self.col < self.viewport_cols && self.col < next_tab_stop {
                        updates.push(self.push_char(' '));
                    }
                }
                '\u{0008}' => {
                    if self.col > 0 {
                        self.col -= 1;
                        if !self.line_buffer.is_empty() {
                            self.line_buffer.pop();
                        }
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

    fn flush_line(&mut self) -> EmulatorResult {
        match self.emit_line_snapshot() {
            Some(snapshot) => vec![snapshot],
            None => Vec::new(),
        }
    }

    fn update_dimensions(&mut self, rows: usize, cols: usize) {
        self.viewport_rows = rows.max(1);
        self.viewport_cols = cols.max(1);
        if self.col >= self.viewport_cols {
            self.col = self.viewport_cols.saturating_sub(1);
        }
        self.line_buffer.truncate(self.viewport_cols);
        self.line_buffer
            .reserve(self.viewport_cols.saturating_sub(self.line_buffer.len()));
    }
}

impl TerminalEmulator for SimpleTerminalEmulator {
    fn handle_output(&mut self, chunk: &[u8], _grid: &TerminalGrid) -> EmulatorResult {
        self.process_chunk(chunk)
    }

    fn flush(&mut self, _grid: &TerminalGrid) -> EmulatorResult {
        self.flush_line()
    }

    fn resize(&mut self, rows: usize, cols: usize) {
        self.update_dimensions(rows, cols);
    }
}

pub struct AlacrittyEmulator {
    inner: SimpleTerminalEmulator,
}

unsafe impl Send for AlacrittyEmulator {}

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

    fn resize(&mut self, rows: usize, cols: usize) {
        self.inner.resize(rows, cols);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::terminal::unpack_cell;

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
                assert_eq!(cell.row, 0);
            }
            _ => panic!("expected cell update"),
        }
    }
}

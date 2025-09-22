use crate::cache::GridCache;
use crate::cache::Seq;
use crate::cache::terminal::{
    PackedCell, Style, StyleId, StyleTable, TerminalGrid, pack_cell, pack_from_heavy,
};
use crate::model::terminal::cell::{Cell as HeavyCell, CellAttributes, Color as HeavyColor};
use crate::model::terminal::diff::{CacheUpdate, CellWrite, RowSnapshot};
use alacritty_terminal::{
    Term,
    event::{Event, EventListener},
    grid::Dimensions,
    index::{Column, Line, Point},
    term::{Config, cell::Cell as AlacrittyCell, cell::Flags as CellFlags},
    vte::ansi::{Color as AnsiColor, NamedColor, Processor},
};
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

struct TermDimensions {
    columns: usize,
    screen_lines: usize,
    total_lines: usize,
}

impl TermDimensions {
    fn new(columns: usize, screen_lines: usize) -> Self {
        Self {
            columns,
            screen_lines,
            total_lines: screen_lines,
        }
    }
}

impl Dimensions for TermDimensions {
    fn total_lines(&self) -> usize {
        self.total_lines
    }

    fn screen_lines(&self) -> usize {
        self.screen_lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

#[derive(Clone, Copy, Default)]
struct EventProxy;

impl EventListener for EventProxy {
    fn send_event(&self, _event: Event) {}
}

pub struct AlacrittyEmulator {
    term: Term<EventProxy>,
    parser: Processor,
    seq: Seq,
    need_full_redraw: bool,
}

unsafe impl Send for AlacrittyEmulator {}

impl AlacrittyEmulator {
    pub fn new(grid: &TerminalGrid) -> Self {
        let (rows, cols) = grid.dims();
        let dimensions = TermDimensions::new(cols.max(1), rows.max(1));
        let config = Config::default();
        let mut term = Term::new(config, &dimensions, EventProxy::default());
        let mut parser = Processor::new();
        // Enable standard LF behavior so shells that rely on ESC[20h behave normally.
        for byte in b"\x1b[20h" {
            parser.advance(&mut term, *byte);
        }
        term.reset_damage();
        Self {
            term,
            parser,
            seq: 0,
            need_full_redraw: true,
        }
    }

    fn next_seq(&mut self) -> Seq {
        self.seq = self.seq.saturating_add(1);
        self.seq
    }

    fn render_full(&mut self, grid: &TerminalGrid) -> EmulatorResult {
        let term_grid = self.term.grid();
        let cols = term_grid.columns();
        let rows = term_grid.screen_lines();
        if cols == 0 || rows == 0 {
            self.need_full_redraw = false;
            return Vec::new();
        }
        let display_offset = term_grid.display_offset();
        let total_lines = term_grid.total_lines();
        let history_size = total_lines.saturating_sub(rows);
        let viewport_top = history_size.saturating_sub(display_offset);
        let style_table = grid.style_table.clone();

        let updates = self.render_full_internal(
            rows,
            cols,
            viewport_top,
            display_offset,
            style_table.as_ref(),
        );
        self.need_full_redraw = false;
        self.term.reset_damage();
        updates
    }

    fn render_full_internal(
        &mut self,
        rows: usize,
        cols: usize,
        viewport_top: usize,
        display_offset: usize,
        style_table: &StyleTable,
    ) -> EmulatorResult {
        let mut updates = Vec::with_capacity(rows);
        for visible_line in 0..rows {
            let absolute_row = viewport_top + visible_line;
            let cells = self.snapshot_visible_line(visible_line, cols, display_offset, style_table);
            let seq = self.next_seq();
            updates.push(CacheUpdate::Row(RowSnapshot::new(absolute_row, seq, cells)));
        }
        updates
    }

    fn snapshot_visible_line(
        &mut self,
        visible_line: usize,
        cols: usize,
        display_offset: usize,
        style_table: &StyleTable,
    ) -> Vec<PackedCell> {
        let mut row = Vec::with_capacity(cols);
        let line_index = visible_line as isize - display_offset as isize;
        let line = Line(line_index as i32);
        let grid = self.term.grid();
        for col in 0..cols {
            let point = Point::new(line, Column(col));
            let cell = &grid[point];
            let heavy = convert_cell(cell);
            row.push(pack_from_heavy(&heavy, style_table));
        }
        row
    }

    fn collect_updates(&mut self, grid: &TerminalGrid) -> EmulatorResult {
        let term_grid = self.term.grid();
        let cols = term_grid.columns();
        let rows = term_grid.screen_lines();
        if cols == 0 || rows == 0 {
            self.term.reset_damage();
            self.need_full_redraw = false;
            return Vec::new();
        }

        let display_offset = term_grid.display_offset();
        let total_lines = term_grid.total_lines();
        let history_size = total_lines.saturating_sub(rows);
        let viewport_top = history_size.saturating_sub(display_offset);
        let forced_full = self.need_full_redraw;
        let mut updates = Vec::new();
        let mut damaged_lines: Vec<usize> = Vec::new();

        match self.term.damage() {
            alacritty_terminal::term::TermDamage::Full => {
                self.need_full_redraw = true;
            }
            alacritty_terminal::term::TermDamage::Partial(iter) if !forced_full => {
                for bounds in iter {
                    let visible_line = bounds.line.saturating_sub(display_offset);
                    if visible_line >= rows || damaged_lines.contains(&visible_line) {
                        continue;
                    }
                    damaged_lines.push(visible_line);
                }
            }
            _ => {}
        }

        self.term.reset_damage();

        if self.need_full_redraw || forced_full {
            let style_table = grid.style_table.clone();
            updates = self.render_full_internal(
                rows,
                cols,
                viewport_top,
                display_offset,
                style_table.as_ref(),
            );
            self.need_full_redraw = false;
        } else if !damaged_lines.is_empty() {
            let style_table = grid.style_table.clone();
            for visible_line in damaged_lines {
                let cells = self.snapshot_visible_line(
                    visible_line,
                    cols,
                    display_offset,
                    style_table.as_ref(),
                );
                let seq = self.next_seq();
                let absolute_row = viewport_top + visible_line;
                updates.push(CacheUpdate::Row(RowSnapshot::new(absolute_row, seq, cells)));
            }
        }

        updates
    }
}

impl TerminalEmulator for AlacrittyEmulator {
    fn handle_output(&mut self, chunk: &[u8], grid: &TerminalGrid) -> EmulatorResult {
        if !chunk.is_empty() {
            for byte in chunk {
                self.parser.advance(&mut self.term, *byte);
            }
        }
        self.collect_updates(grid)
    }

    fn flush(&mut self, grid: &TerminalGrid) -> EmulatorResult {
        self.need_full_redraw = true;
        self.render_full(grid)
    }

    fn resize(&mut self, rows: usize, cols: usize) {
        let dims = TermDimensions::new(cols.max(1), rows.max(1));
        self.term.resize(dims);
        self.need_full_redraw = true;
    }
}

fn convert_cell(cell: &AlacrittyCell) -> HeavyCell {
    HeavyCell {
        char: cell.c,
        fg_color: convert_color(&cell.fg),
        bg_color: convert_color(&cell.bg),
        attributes: convert_attributes(cell.flags),
    }
}

fn convert_color(color: &AnsiColor) -> HeavyColor {
    match color {
        AnsiColor::Spec(rgb) => HeavyColor::Rgb(rgb.r, rgb.g, rgb.b),
        AnsiColor::Indexed(idx) => HeavyColor::Indexed(*idx),
        AnsiColor::Named(name) => match name {
            NamedColor::Foreground
            | NamedColor::BrightForeground
            | NamedColor::DimForeground
            | NamedColor::Background
            | NamedColor::Cursor => HeavyColor::Default,
            other => {
                let value = *other as usize;
                if value <= u8::MAX as usize {
                    HeavyColor::Indexed(value as u8)
                } else {
                    HeavyColor::Default
                }
            }
        },
    }
}

fn convert_attributes(flags: CellFlags) -> CellAttributes {
    CellAttributes {
        bold: flags.contains(CellFlags::BOLD)
            || flags.contains(CellFlags::DIM_BOLD)
            || flags.contains(CellFlags::BOLD_ITALIC),
        italic: flags.contains(CellFlags::ITALIC) || flags.contains(CellFlags::BOLD_ITALIC),
        underline: flags.intersects(CellFlags::ALL_UNDERLINES),
        strikethrough: flags.contains(CellFlags::STRIKEOUT),
        reverse: flags.contains(CellFlags::INVERSE),
        blink: false,
        dim: flags.contains(CellFlags::DIM) || flags.contains(CellFlags::DIM_BOLD),
        hidden: flags.contains(CellFlags::HIDDEN),
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

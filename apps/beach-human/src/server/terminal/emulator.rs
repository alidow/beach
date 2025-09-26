use crate::cache::Seq;
use crate::cache::terminal::{
    PackedCell, Style, StyleId, StyleTable, TerminalGrid, attrs_to_byte, pack_cell,
    pack_color_from_heavy,
};
use crate::model::terminal::cell::{Cell as HeavyCell, CellAttributes, Color as HeavyColor};
use crate::model::terminal::diff::{
    CacheUpdate, CellWrite, HistoryTrim, RowSnapshot, StyleDefinition,
};
use alacritty_terminal::{
    Term,
    event::{Event, EventListener},
    grid::Dimensions,
    index::{Column, Line, Point},
    term::{Config, cell::Cell as AlacrittyCell, cell::Flags as CellFlags},
    vte::ansi::{Color as AnsiColor, NamedColor, Processor},
};
use std::borrow::Cow;
use std::convert::TryFrom;
use tracing::trace;

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
    absolute_row: u64,
    origin_row: u64,
    col: usize,
    seq: Seq,
    default_style: StyleId,
    line_buffer: Vec<PackedCell>,
}

unsafe impl Send for SimpleTerminalEmulator {}

impl SimpleTerminalEmulator {
    pub fn new(grid: &TerminalGrid) -> Self {
        let (viewport_rows, viewport_cols) = grid.viewport_size();
        let viewport_rows = viewport_rows.max(1);
        let viewport_cols = viewport_cols.max(1);
        let default_style = grid.ensure_style_id(Style::default());
        let absolute_row = grid.row_offset();
        Self {
            viewport_rows,
            viewport_cols,
            absolute_row,
            origin_row: absolute_row,
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

    fn relative_row(&self) -> usize {
        let rel = self.absolute_row.saturating_sub(self.origin_row);
        rel.min(usize::MAX as u64) as usize
    }

    fn push_char(&mut self, ch: char) -> CacheUpdate {
        if self.col >= self.viewport_cols {
            self.advance_row();
        }
        self.seq = self.seq.saturating_add(1);
        let cell = pack_cell(ch, self.default_style);
        let row = self.relative_row();
        let update = CacheUpdate::Cell(CellWrite::new(row, self.col, self.seq, cell));
        self.col = self.col.saturating_add(1);
        update
    }

    fn emit_line_snapshot(&mut self) -> Option<CacheUpdate> {
        if self.line_buffer.is_empty() {
            None
        } else {
            self.seq = self.seq.saturating_add(1);
            let row = self.relative_row();
            let snapshot = RowSnapshot::new(row, self.seq, self.line_buffer.clone());
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
    session_origin: Option<u64>,
    last_seeded_tail: Option<usize>,
}

unsafe impl Send for AlacrittyEmulator {}

impl AlacrittyEmulator {
    pub fn new(grid: &TerminalGrid) -> Self {
        let (viewport_rows, viewport_cols) = grid.viewport_size();
        let dimensions = TermDimensions::new(viewport_cols.max(1), viewport_rows.max(1));
        let mut config = Config::default();
        config.scrolling_history = grid.history_limit();
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
            session_origin: None,
            last_seeded_tail: None,
        }
    }

    fn next_seq(&mut self) -> Seq {
        self.seq = self.seq.saturating_add(1);
        self.seq
    }

    fn ensure_session_origin(&mut self, base_row: u64, viewport_top: usize) -> u64 {
        let candidate = base_row.saturating_add(viewport_top as u64);
        match self.session_origin {
            Some(origin) if candidate <= origin => origin,
            _ => {
                self.session_origin = Some(candidate);
                self.last_seeded_tail = None;
                candidate
            }
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn rebase_row(&self, absolute_row: usize) -> usize {
        let origin = self.session_origin.unwrap_or(0);
        let abs = absolute_row as u64;
        let rel = abs.saturating_sub(origin);
        rel.min(usize::MAX as u64) as usize
    }

    fn tail_blank_candidate(
        &self,
        base_row: u64,
        viewport_top: usize,
        rows: usize,
    ) -> Option<(usize, usize)> {
        if rows == 0 {
            return None;
        }
        let tail_index = rows.saturating_sub(1);
        let tail_absolute = Self::absolute_row_id(base_row, viewport_top, tail_index)? as u64;
        let next_absolute = tail_absolute.saturating_add(1);
        let origin = self.session_origin.unwrap_or(0);
        let relative = next_absolute.saturating_sub(origin);
        let relative = relative.min(usize::MAX as u64) as usize;
        let absolute = match usize::try_from(next_absolute) {
            Ok(value) => value,
            Err(_) => return None,
        };
        Some((relative, absolute))
    }

    fn seed_tail_blank_row(
        &mut self,
        candidate: Option<(usize, usize)>,
        cell_updates: &mut Vec<CacheUpdate>,
    ) {
        let Some((next_relative, next_absolute)) = candidate else {
            return;
        };
        if self.last_seeded_tail == Some(next_relative) {
            return;
        }
        let seq = self.next_seq();
        let blank = pack_cell(' ', StyleId::DEFAULT);
        cell_updates.push(CacheUpdate::Cell(CellWrite::new(
            next_absolute,
            0,
            seq,
            blank,
        )));
        self.last_seeded_tail = Some(next_relative);
    }

    fn consume_trim_events(&self, grid: &TerminalGrid, out: &mut Vec<CacheUpdate>) {
        for event in grid.drain_trim_events() {
            match usize::try_from(event.start) {
                Ok(start) => {
                    out.push(CacheUpdate::Trim(HistoryTrim::new(start, event.count)));
                    trace!(
                        target = "server::grid",
                        trim_start = start,
                        trim_count = event.count,
                        marker = "tail_base_row_v3"
                    );
                }
                Err(_) => {
                    trace!(
                        target = "server::grid",
                        trim_start = ?event.start,
                        trim_count = event.count,
                        marker = "tail_base_row_v3_skip"
                    );
                }
            }
        }
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
        let mut base_row = grid.row_offset();
        let origin = self.ensure_session_origin(base_row, viewport_top);
        if origin != base_row {
            grid.set_row_offset(origin);
            base_row = origin;
        }

        let mut updates = self.render_full_internal(
            rows,
            cols,
            base_row,
            viewport_top,
            display_offset,
            style_table.as_ref(),
        );
        self.consume_trim_events(grid, &mut updates);
        self.need_full_redraw = false;
        self.term.reset_damage();
        updates
    }

    fn render_full_internal(
        &mut self,
        rows: usize,
        cols: usize,
        base_row: u64,
        viewport_top: usize,
        display_offset: usize,
        style_table: &StyleTable,
    ) -> EmulatorResult {
        let mut style_updates = Vec::new();
        let mut emitted_styles = std::collections::HashSet::new();
        let mut cell_updates = Vec::with_capacity(rows * cols);
        for visible_line in 0..rows {
            let Some(absolute_row) = Self::absolute_row_id(base_row, viewport_top, visible_line)
            else {
                continue;
            };
            let line_index = visible_line as isize - display_offset as isize;
            let line = Line(line_index as i32);
            for col in 0..cols {
                let point = Point::new(line, Column(col));
                let (packed, style_id, style, is_new) = self.pack_point(point, style_table);
                if is_new || emitted_styles.insert(style_id.0) {
                    let seq = self.next_seq();
                    style_updates.push(CacheUpdate::Style(StyleDefinition::new(
                        style_id, seq, style,
                    )));
                }
                let seq = self.next_seq();
                cell_updates.push(CacheUpdate::Cell(CellWrite::new(
                    absolute_row,
                    col,
                    seq,
                    packed,
                )));
            }
        }
        let tail_candidate = self.tail_blank_candidate(base_row, viewport_top, rows);
        self.seed_tail_blank_row(tail_candidate, &mut cell_updates);
        style_updates.extend(cell_updates);
        style_updates
    }

    #[inline]
    fn absolute_row_id(base_row: u64, viewport_top: usize, visible_line: usize) -> Option<usize> {
        let relative = viewport_top.checked_add(visible_line)? as u64;
        let absolute = base_row.checked_add(relative)?;
        usize::try_from(absolute).ok()
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
        let mut base_row = grid.row_offset();
        let forced_full = self.need_full_redraw;
        let origin = self.ensure_session_origin(base_row, viewport_top);
        if origin != base_row {
            grid.set_row_offset(origin);
            base_row = origin;
        }
        let mut cell_updates = Vec::new();
        let mut style_updates = Vec::new();
        let mut emitted_styles = std::collections::HashSet::new();
        let mut damaged_lines: Vec<(usize, usize, usize)> = Vec::new();
        let mut touched_cells = 0usize;

        match self.term.damage() {
            alacritty_terminal::term::TermDamage::Full => {
                self.need_full_redraw = true;
            }
            alacritty_terminal::term::TermDamage::Partial(iter) if !forced_full => {
                for bounds in iter {
                    let visible_line = bounds.line.saturating_sub(display_offset);
                    if visible_line >= rows {
                        continue;
                    }
                    let left = bounds.left.min(cols.saturating_sub(1));
                    let right = bounds.right.min(cols.saturating_sub(1));
                    if left > right {
                        continue;
                    }
                    touched_cells =
                        touched_cells.saturating_add(right.saturating_sub(left).saturating_add(1));
                    damaged_lines.push((visible_line, left, right));
                }
            }
            _ => {}
        }

        self.term.reset_damage();

        if self.need_full_redraw || forced_full || touched_cells >= rows.saturating_mul(cols) / 2 {
            let style_table = grid.style_table.clone();
            let updates = self.render_full_internal(
                rows,
                cols,
                base_row,
                viewport_top,
                display_offset,
                style_table.as_ref(),
            );
            style_updates.extend(updates);
            self.need_full_redraw = false;
        } else if !damaged_lines.is_empty() {
            let style_table = grid.style_table.clone();
            for (visible_line, left, right) in damaged_lines {
                let Some(absolute_row) =
                    Self::absolute_row_id(base_row, viewport_top, visible_line)
                else {
                    continue;
                };
                let line_index = visible_line as isize - display_offset as isize;
                let line = Line(line_index as i32);
                for col in left..=right {
                    let point = Point::new(line, Column(col));
                    let (packed, style_id, style, is_new) =
                        self.pack_point(point, style_table.as_ref());
                    if is_new && emitted_styles.insert(style_id.0) {
                        let seq = self.next_seq();
                        style_updates.push(CacheUpdate::Style(StyleDefinition::new(
                            style_id, seq, style,
                        )));
                    }
                    let seq = self.next_seq();
                    cell_updates.push(CacheUpdate::Cell(CellWrite::new(
                        absolute_row,
                        col,
                        seq,
                        packed,
                    )));
                }
            }
        }

        if !(self.need_full_redraw || forced_full || touched_cells >= rows.saturating_mul(cols) / 2)
        {
            let tail_candidate = self.tail_blank_candidate(base_row, viewport_top, rows);
            self.seed_tail_blank_row(tail_candidate, &mut cell_updates);
        }

        style_updates.extend(cell_updates);
        self.consume_trim_events(grid, &mut style_updates);
        style_updates
    }
}

impl AlacrittyEmulator {
    fn pack_point(
        &mut self,
        point: Point,
        style_table: &StyleTable,
    ) -> (PackedCell, StyleId, Style, bool) {
        let grid = self.term.grid();
        let cell = &grid[point];
        let heavy = convert_cell(cell);
        let style = style_from_heavy(&heavy);
        let (style_id, is_new) = style_table.ensure_id_with_new(style);
        let packed = pack_cell(heavy.char, style_id);
        (packed, style_id, style, is_new)
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

fn style_from_heavy(cell: &HeavyCell) -> Style {
    Style {
        fg: pack_color_from_heavy(&cell.fg_color),
        bg: pack_color_from_heavy(&cell.bg_color),
        attrs: attrs_to_byte(&cell.attributes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::terminal::unpack_cell;

    #[test_timeout::timeout]
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

    #[test_timeout::timeout]
    fn session_origin_updates_when_viewport_shifts() {
        let grid = TerminalGrid::new(24, 80);
        let mut emulator = AlacrittyEmulator::new(&grid);

        // Initial call sees no scrollback.
        let origin0 = emulator.ensure_session_origin(0, 0);
        assert_eq!(origin0, 0, "expected initial origin to be zero");

        // Later the viewport reveals that the PTY started at row 160.
        let origin1 = emulator.ensure_session_origin(0, 160);
        assert_eq!(
            origin1, 160,
            "origin should update to the first absolute row"
        );

        // Rows rebased after the adjustment should now anchor at zero.
        let relative = emulator.rebase_row(160);
        assert_eq!(
            relative, 0,
            "row 160 should map to relative zero after rebasing"
        );
    }

    #[test_timeout::timeout]
    fn newline_triggers_tail_blank_seed() {
        let grid = TerminalGrid::new(4, 10);
        let mut emulator = AlacrittyEmulator::new(&grid);

        let updates = emulator.handle_output(b"hello\n", &grid);
        let mut saw_blank = false;
        for update in updates {
            if let CacheUpdate::Cell(cell) = update {
                let (ch, _) = unpack_cell(cell.cell);
                if cell.row == 1 && cell.col == 0 && ch == ' ' {
                    saw_blank = true;
                    break;
                }
            }
        }

        assert!(saw_blank, "expected blank cell for tail row");
    }
}

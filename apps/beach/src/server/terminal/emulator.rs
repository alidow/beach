use crate::cache::Seq;
use crate::cache::terminal::{
    PackedCell, Style, StyleId, StyleTable, TerminalGrid, attrs_to_byte, pack_cell,
    pack_color_from_heavy, unpack_cell,
};
use crate::model::terminal::CursorState;
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
    vte::ansi::{Color as AnsiColor, CursorShape, NamedColor, Processor},
};
use std::borrow::Cow;
use std::collections::HashSet;
use std::convert::TryFrom;
use tracing::{Level, trace};

pub type EmulatorResult = Vec<CacheUpdate>;

pub trait TerminalEmulator: Send {
    fn handle_output(&mut self, chunk: &[u8], grid: &TerminalGrid) -> EmulatorResult;
    fn flush(&mut self, grid: &TerminalGrid) -> EmulatorResult {
        let _ = grid;
        Vec::new()
    }
    fn resize(&mut self, rows: usize, cols: usize);
}

#[derive(Default)]
struct GridSnapshot {
    base_row: Option<u64>,
    rows: Vec<Vec<PackedCell>>,
}

impl GridSnapshot {
    fn clear(&mut self) {
        self.base_row = None;
        self.rows.clear();
    }

    fn replace(&mut self, base_row: u64, rows: Vec<Vec<PackedCell>>) {
        self.base_row = Some(base_row);
        self.rows = rows;
    }

    fn row(&self, absolute: u64) -> Option<&[PackedCell]> {
        let base = self.base_row?;
        if absolute < base {
            return None;
        }
        let idx = (absolute - base) as usize;
        self.rows.get(idx).map(|row| row.as_slice())
    }
}

struct CapturedRow {
    absolute: u64,
    cells: Vec<PackedCell>,
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
    session_origin: Option<u64>,
    snapshot: GridSnapshot,
    cursor_frames_enabled: bool,
    last_cursor: Option<CursorState>,
}

unsafe impl Send for AlacrittyEmulator {}

impl AlacrittyEmulator {
    pub fn new(grid: &TerminalGrid, cursor_frames_enabled: bool) -> Self {
        let (viewport_rows, viewport_cols) = grid.viewport_size();
        let dimensions = TermDimensions::new(viewport_cols.max(1), viewport_rows.max(1));
        let config = Config {
            scrolling_history: grid.history_limit(),
            ..Config::default()
        };
        let mut term = Term::new(config, &dimensions, EventProxy);
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
            session_origin: None,
            snapshot: GridSnapshot::default(),
            cursor_frames_enabled,
            last_cursor: None,
        }
    }

    fn next_seq(&mut self) -> Seq {
        self.seq = self.seq.saturating_add(1);
        self.seq
    }

    fn ensure_session_origin(&mut self, base_row: u64, viewport_top: usize) -> u64 {
        let candidate = base_row.saturating_add(viewport_top as u64);
        let previous = self.session_origin;
        let origin = match previous {
            Some(origin) if candidate <= origin => origin,
            _ => {
                self.session_origin = Some(candidate);
                candidate
            }
        };
        trace!(
            target = "server::emulator",
            base_row,
            viewport_top,
            candidate,
            origin,
            prev_origin = previous.unwrap_or(origin),
            updated = previous != Some(origin),
            "ensure_session_origin"
        );
        origin
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn rebase_row(&self, absolute_row: usize) -> usize {
        let origin = self.session_origin.unwrap_or(0);
        let abs = absolute_row as u64;
        let rel = abs.saturating_sub(origin);
        rel.min(usize::MAX as u64) as usize
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

    fn capture_full_grid(&mut self, grid: &TerminalGrid) -> (Vec<CapturedRow>, Vec<CacheUpdate>) {
        let (cols, total_lines, screen_lines, display_offset, top_line, bottom_line) = {
            let term_grid = self.term.grid();
            (
                term_grid.columns(),
                term_grid.total_lines(),
                term_grid.screen_lines(),
                term_grid.display_offset(),
                term_grid.topmost_line().0,
                term_grid.bottommost_line().0,
            )
        };
        if cols == 0 || total_lines == 0 {
            return (Vec::new(), Vec::new());
        }

        let history_size = total_lines.saturating_sub(screen_lines);
        let viewport_top = history_size.saturating_sub(display_offset);
        let base_row = grid.row_offset();
        let origin = self.ensure_session_origin(base_row, viewport_top);
        trace!(
            target = "server::emulator",
            cols,
            total_lines,
            screen_lines,
            display_offset,
            top_line,
            bottom_line,
            history_size,
            viewport_top,
            base_row,
            origin,
            "capture_full_grid window"
        );

        let style_table = grid.style_table.clone();
        let mut style_updates = Vec::new();
        let mut emitted_styles = HashSet::new();
        let mut captured_rows = Vec::with_capacity((bottom_line - top_line + 1) as usize);

        for line_idx in top_line..=bottom_line {
            let line = Line(line_idx);
            let absolute = if line_idx >= 0 {
                origin.saturating_add(line_idx as u64)
            } else {
                match origin.checked_sub((-line_idx) as u64) {
                    Some(value) => value,
                    None => {
                        trace!(
                            target = "server::emulator",
                            origin,
                            line_idx,
                            "capture_full_grid skipped_negative_line"
                        );
                        continue;
                    }
                }
            };

            let mut row_cells = Vec::with_capacity(cols);
            for col in 0..cols {
                let point = Point::new(line, Column(col));
                let (packed, style_id, style, is_new) =
                    self.pack_point(point, style_table.as_ref());
                if is_new || emitted_styles.insert(style_id.0) {
                    let seq = self.next_seq();
                    style_updates.push(CacheUpdate::Style(StyleDefinition::new(
                        style_id, seq, style,
                    )));
                }
                row_cells.push(packed);
            }

            captured_rows.push(CapturedRow {
                absolute,
                cells: row_cells,
            });
        }

        self.term.reset_damage();
        (captured_rows, style_updates)
    }

    fn emit_deltas(&mut self, captured: Vec<CapturedRow>) -> EmulatorResult {
        if captured.is_empty() {
            self.snapshot.clear();
            return Vec::new();
        }

        let mut updates = Vec::new();

        for row in &captured {
            let Some(prev_cells) = self.snapshot.row(row.absolute) else {
                if let Ok(row_idx) = usize::try_from(row.absolute) {
                    let seq = self.next_seq();
                    updates.push(CacheUpdate::Row(RowSnapshot::new(
                        row_idx,
                        seq,
                        row.cells.clone(),
                    )));
                }
                continue;
            };

            if prev_cells.len() != row.cells.len() {
                if let Ok(row_idx) = usize::try_from(row.absolute) {
                    let seq = self.next_seq();
                    updates.push(CacheUpdate::Row(RowSnapshot::new(
                        row_idx,
                        seq,
                        row.cells.clone(),
                    )));
                }
                continue;
            }

            let mut diff_cells = Vec::new();
            for (col, (&prev, &curr)) in prev_cells.iter().zip(&row.cells).enumerate() {
                if prev != curr {
                    diff_cells.push((col, curr));
                }
            }

            if diff_cells.is_empty() {
                continue;
            }

            if diff_cells.len() > row.cells.len() / 2 {
                if let Ok(row_idx) = usize::try_from(row.absolute) {
                    let seq = self.next_seq();
                    updates.push(CacheUpdate::Row(RowSnapshot::new(
                        row_idx,
                        seq,
                        row.cells.clone(),
                    )));
                }
            } else if let Ok(row_idx) = usize::try_from(row.absolute) {
                for (col, cell) in diff_cells {
                    let seq = self.next_seq();
                    updates.push(CacheUpdate::Cell(CellWrite::new(row_idx, col, seq, cell)));
                }
            }
        }

        let base_row = captured
            .first()
            .map(|row| row.absolute)
            .unwrap_or(self.snapshot.base_row.unwrap_or(0));
        let snapshot_rows = captured.into_iter().map(|row| row.cells).collect();
        self.snapshot.replace(base_row, snapshot_rows);

        updates
    }

    fn collect_full_diff(&mut self, grid: &TerminalGrid) -> EmulatorResult {
        let (captured_rows, mut style_updates) = self.capture_full_grid(grid);
        let mut updates = self.emit_deltas(captured_rows);
        if !style_updates.is_empty() {
            style_updates.extend(updates);
            updates = style_updates;
        }
        self.consume_trim_events(grid, &mut updates);
        self.push_cursor_update(grid, &mut updates);
        updates
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

    fn push_cursor_update(&mut self, grid: &TerminalGrid, updates: &mut Vec<CacheUpdate>) {
        if !self.cursor_frames_enabled {
            return;
        }

        let Some((row, col, visible, blink)) = self.compute_cursor_components(grid) else {
            return;
        };

        let target_seq = if updates.is_empty() {
            self.seq.saturating_add(1)
        } else {
            self.seq
        };

        let cursor = CursorState::new(row, col, target_seq, visible, blink);

        let should_emit = match self.last_cursor {
            Some(prev) => {
                cursor.seq > prev.seq
                    || cursor.row != prev.row
                    || cursor.col != prev.col
                    || cursor.visible != prev.visible
                    || cursor.blink != prev.blink
            }
            None => true,
        };

        if !should_emit {
            return;
        }

        if updates.is_empty() {
            self.seq = target_seq;
        }

        self.last_cursor = Some(cursor);
        updates.push(CacheUpdate::Cursor(cursor));
    }

    fn compute_cursor_components(
        &mut self,
        grid: &TerminalGrid,
    ) -> Option<(usize, usize, bool, bool)> {
        let origin = {
            let term_grid = self.term.grid();
            let total_lines = term_grid.total_lines();
            let screen_lines = term_grid.screen_lines();
            let display_offset = term_grid.display_offset();
            let history_size = total_lines.saturating_sub(screen_lines);
            let viewport_top = history_size.saturating_sub(display_offset);
            let base_row = grid.row_offset();
            self.ensure_session_origin(base_row, viewport_top)
        };

        let render_cursor = self.term.renderable_content().cursor;
        let cursor_style = self.term.cursor_style();

        let line = render_cursor.point.line.0;
        let delta = if line >= 0 {
            line as u64
        } else {
            line.wrapping_neg() as u64
        };
        let absolute_row = if line >= 0 {
            origin.saturating_add(delta)
        } else {
            origin.saturating_sub(delta)
        };
        let row = usize::try_from(absolute_row).ok()?;
        let col = render_cursor.point.column.0;
        let visible = render_cursor.shape != CursorShape::Hidden;
        let blink = cursor_style.blinking;

        if tracing::enabled!(Level::TRACE) {
            if let Some(index) = grid.index_of_row(absolute_row) {
                let cols = grid.cols().max(1);
                let mut packed = vec![0u64; cols];
                if grid.snapshot_row_into(index, &mut packed).is_ok() {
                    let mut committed_width = 0usize;
                    let mut preview = String::new();
                    for (idx, raw) in packed.iter().enumerate() {
                        let (ch, _) = unpack_cell(PackedCell::from(*raw));
                        if ch != ' ' {
                            committed_width = idx + 1;
                        }
                        if preview.len() < 32 {
                            preview.push(ch);
                        }
                    }
                    let preview_trimmed = preview.trim_end_matches(' ').to_string();
                    trace!(
                        target = "server::cursor",
                        cursor_row = row,
                        cursor_col = col,
                        committed_width,
                        preview = %preview_trimmed,
                        marker = "cursor_components"
                    );
                } else {
                    trace!(
                        target = "server::cursor",
                        cursor_row = row,
                        cursor_col = col,
                        marker = "cursor_row_snapshot_failed"
                    );
                }
            } else {
                trace!(
                    target = "server::cursor",
                    cursor_row = row,
                    cursor_col = col,
                    marker = "cursor_row_unloaded"
                );
            }
        }

        Some((row, col, visible, blink))
    }
}

impl TerminalEmulator for AlacrittyEmulator {
    fn handle_output(&mut self, chunk: &[u8], grid: &TerminalGrid) -> EmulatorResult {
        if !chunk.is_empty() {
            for byte in chunk {
                self.parser.advance(&mut self.term, *byte);
            }
        }
        self.collect_full_diff(grid)
    }

    fn flush(&mut self, grid: &TerminalGrid) -> EmulatorResult {
        self.collect_full_diff(grid)
    }

    fn resize(&mut self, rows: usize, cols: usize) {
        let dims = TermDimensions::new(cols.max(1), rows.max(1));
        self.term.resize(dims);
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
    use crate::server::terminal::apply_update;

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
        let mut emulator = AlacrittyEmulator::new(&grid, false);

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
    fn alacritty_emits_scrollback_cells() {
        let grid = TerminalGrid::new(24, 80);
        let mut emulator = AlacrittyEmulator::new(&grid, false);

        let burst = (1..=150)
            .map(|i| format!("Line {i}: Test\n"))
            .collect::<String>();

        let updates = emulator.handle_output(burst.as_bytes(), &grid);
        for update in &updates {
            apply_update(&grid, update);
        }

        assert!(
            grid_contains(&grid, "Line 1: Test"),
            "missing earliest line"
        );
        assert!(
            grid_contains(&grid, "Line 150: Test"),
            "missing latest line"
        );
    }

    fn grid_contains(grid: &TerminalGrid, needle: &str) -> bool {
        let mut buffer = vec![0u64; grid.cols()];
        let first = grid.first_row_id().unwrap_or(0);
        let last = grid.last_row_id().unwrap_or(first);
        for absolute in first..=last {
            if let Some(index) = grid.index_of_row(absolute) {
                if grid.snapshot_row_into(index, &mut buffer).is_ok() {
                    let text: String = buffer
                        .iter()
                        .map(|cell| unpack_cell(PackedCell::from(*cell)).0)
                        .collect();
                    if text.trim_end().contains(needle) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

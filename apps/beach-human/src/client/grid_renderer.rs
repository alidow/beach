use crate::cache::Seq;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::collections::HashMap;

/// Minimal style-aware cell state tracked by the unified grid renderer.
#[derive(Clone, Copy, Debug)]
struct CellState {
    ch: char,
    style_id: Option<u32>,
    seq: Seq,
}

impl CellState {
    fn blank() -> Self {
        Self {
            ch: ' ',
            style_id: None,
            seq: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectionPosition {
    pub row: usize,
    pub col: usize,
}

#[derive(Clone, Debug)]
struct SelectionRange {
    anchor: SelectionPosition,
    head: SelectionPosition,
}

#[derive(Clone, Copy, Debug)]
struct PredictedCell {
    ch: char,
    seq: Seq,
}

impl SelectionRange {
    fn new(anchor: SelectionPosition, head: SelectionPosition) -> Self {
        Self { anchor, head }
    }

    fn bounds(&self) -> (SelectionPosition, SelectionPosition) {
        if self.anchor.row < self.head.row
            || (self.anchor.row == self.head.row && self.anchor.col <= self.head.col)
        {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    fn contains(&self, pos: SelectionPosition) -> bool {
        let (start, end) = self.bounds();
        if pos.row < start.row || pos.row > end.row {
            return false;
        }
        if start.row == end.row {
            return pos.col >= start.col && pos.col <= end.col;
        }
        if pos.row == start.row {
            return pos.col >= start.col;
        }
        if pos.row == end.row {
            return pos.col <= end.col;
        }
        true
    }
}

/// Unified grid renderer with scrollback, basic selection, and status overlays.
pub struct GridRenderer {
    rows: usize,
    cols: usize,
    cells: Vec<Vec<CellState>>,
    scroll_top: usize,
    viewport_height: usize,
    follow_tail: bool,
    selection: Option<SelectionRange>,
    needs_redraw: bool,
    predictions: HashMap<(usize, usize), PredictedCell>,
}

impl GridRenderer {
    pub fn new(rows: usize, cols: usize) -> Self {
        let mut renderer = Self {
            rows,
            cols,
            cells: Vec::new(),
            scroll_top: 0,
            viewport_height: 0,
            follow_tail: true,
            selection: None,
            needs_redraw: true,
            predictions: HashMap::new(),
        };
        renderer.ensure_size(rows, cols);
        renderer
    }

    pub fn ensure_size(&mut self, rows: usize, cols: usize) {
        if rows > self.rows {
            for _ in self.rows..rows {
                self.cells
                    .push(vec![CellState::blank(); self.cols.max(cols).max(1)]);
            }
            self.rows = rows;
        }

        if cols > self.cols {
            for row in &mut self.cells {
                row.resize(cols, CellState::blank());
            }
            self.cols = cols;
        }

        if self.cells.len() < self.rows {
            for _ in self.cells.len()..self.rows {
                self.cells.push(vec![CellState::blank(); self.cols.max(1)]);
            }
        }
    }

    pub fn apply_cell(
        &mut self,
        row: usize,
        col: usize,
        seq: Seq,
        ch: char,
        style_id: Option<u32>,
    ) {
        self.ensure_size(row + 1, col + 1);
        self.clear_prediction_at(row, col);
        let cell = &mut self.cells[row][col];
        if seq >= cell.seq {
            cell.ch = ch;
            cell.seq = seq;
            cell.style_id = style_id;
            self.mark_dirty();
        }
        if self.follow_tail {
            self.scroll_to_tail();
        }
    }

    pub fn apply_row_from_text(&mut self, row: usize, seq: Seq, text: &str) {
        let width = text.chars().count();
        self.ensure_size(row + 1, self.cols.max(width));
        for (col, ch) in text.chars().enumerate() {
            self.apply_cell(row, col, seq, ch, None);
        }
        for col in width..self.cols {
            let cell = &mut self.cells[row][col];
            if seq >= cell.seq {
                cell.ch = ' ';
                cell.seq = seq;
                self.mark_dirty();
            }
            self.clear_prediction_at(row, col);
        }
    }

    pub fn apply_row_from_cells(&mut self, row: usize, seq: Seq, cells: &[(char, Option<u32>)]) {
        let width = cells.len();
        self.ensure_size(row + 1, self.cols.max(width));
        for (col, (ch, style_id)) in cells.iter().enumerate() {
            self.apply_cell(row, col, seq, *ch, *style_id);
        }
        for col in width..self.cols {
            let cell = &mut self.cells[row][col];
            if seq >= cell.seq {
                cell.ch = ' ';
                cell.seq = seq;
                cell.style_id = None;
                self.mark_dirty();
            }
            self.clear_prediction_at(row, col);
        }
    }

    pub fn apply_rect(
        &mut self,
        rows: std::ops::Range<usize>,
        cols: std::ops::Range<usize>,
        seq: Seq,
        ch: char,
        style_id: Option<u32>,
    ) {
        self.ensure_size(rows.end, cols.end);
        for row in rows.clone() {
            for col in cols.clone() {
                self.apply_cell(row, col, seq, ch, style_id);
            }
        }
        self.mark_dirty();
    }

    pub fn add_prediction(&mut self, row: usize, col: usize, seq: Seq, ch: char) {
        self.ensure_size(row + 1, col + 1);
        self.predictions
            .insert((row, col), PredictedCell { ch, seq });
        self.mark_dirty();
    }

    pub fn clear_prediction_seq(&mut self, seq: Seq) {
        let before = self.predictions.len();
        self.predictions.retain(|_, cell| cell.seq != seq);
        if self.predictions.len() != before {
            self.mark_dirty();
        }
    }

    pub fn clear_all_predictions(&mut self) {
        if !self.predictions.is_empty() {
            self.predictions.clear();
            self.mark_dirty();
        }
    }

    fn clear_prediction_at(&mut self, row: usize, col: usize) {
        if self.predictions.remove(&(row, col)).is_some() {
            self.mark_dirty();
        }
    }

    pub fn scroll_lines(&mut self, delta: isize) {
        if self.viewport_height == 0 {
            return;
        }
        let max_scroll = self.rows.saturating_sub(self.viewport_height);
        if delta.is_positive() {
            let delta = delta as usize;
            self.scroll_top = (self.scroll_top + delta).min(max_scroll);
        } else {
            let delta = delta.wrapping_abs() as usize;
            self.scroll_top = self.scroll_top.saturating_sub(delta);
        }
        self.follow_tail = self.scroll_top >= max_scroll;
        self.mark_dirty();
    }

    pub fn scroll_pages(&mut self, delta_pages: isize) {
        if self.viewport_height == 0 {
            return;
        }
        let delta = delta_pages * self.viewport_height as isize;
        self.scroll_lines(delta);
    }

    pub fn scroll_to_tail(&mut self) {
        if self.viewport_height == 0 || self.rows == 0 {
            self.scroll_top = 0;
            return;
        }
        self.scroll_top = self.rows.saturating_sub(self.viewport_height);
        self.follow_tail = true;
        self.mark_dirty();
    }

    pub fn scroll_to_top(&mut self) {
        self.follow_tail = false;
        self.scroll_top = 0;
        self.mark_dirty();
    }

    pub fn set_follow_tail(&mut self, follow: bool) {
        self.follow_tail = follow;
        if follow {
            self.scroll_to_tail();
        }
    }

    pub fn toggle_follow_tail(&mut self) {
        let follow = !self.follow_tail;
        self.set_follow_tail(follow);
    }

    pub fn viewport_top(&self) -> usize {
        self.scroll_top
    }

    pub fn viewport_height(&self) -> usize {
        self.viewport_height.max(1)
    }

    pub fn total_rows(&self) -> usize {
        self.rows
    }

    pub fn total_cols(&self) -> usize {
        self.cols
    }

    pub fn clamp_position(&self, row: isize, col: isize) -> SelectionPosition {
        if self.rows == 0 || self.cols == 0 {
            return SelectionPosition { row: 0, col: 0 };
        }
        let max_row = (self.rows - 1) as isize;
        let clamped_row = row.clamp(0, max_row) as usize;
        let mut max_col = (self.cols - 1) as isize;
        let row_width = self.row_display_width(clamped_row);
        if row_width > 0 {
            max_col = max_col.min((row_width - 1) as isize);
        }
        let clamped_col = col.clamp(0, max_col.max(0)) as usize;
        SelectionPosition {
            row: clamped_row,
            col: clamped_col,
        }
    }

    pub fn selection_text(&self) -> Option<String> {
        let range = self.selection.as_ref()?;
        if self.rows == 0 || self.cols == 0 {
            return None;
        }
        let (start, end) = range.bounds();
        let mut output = String::new();
        for row in start.row..=end.row {
            if row >= self.rows {
                break;
            }
            let mut row_start = if row == start.row { start.col } else { 0 };
            let mut row_end = if row == end.row {
                end.col
            } else {
                self.cols.saturating_sub(1)
            };
            row_start = row_start.min(self.cols.saturating_sub(1));
            row_end = row_end.min(self.cols.saturating_sub(1));
            if row_end < row_start {
                continue;
            }
            for col in row_start..=row_end {
                let (ch, _, _) = self.cell_for_render(row, col);
                output.push(ch);
            }
            if row != end.row {
                output.push('\n');
            }
        }
        Some(output)
    }

    pub fn row_display_width(&self, row: usize) -> usize {
        if row >= self.rows {
            return 0;
        }
        self.cells[row]
            .iter()
            .rposition(|cell| cell.ch != ' ')
            .map(|idx| idx + 1)
            .unwrap_or(0)
    }

    pub fn set_selection(&mut self, anchor: SelectionPosition, head: SelectionPosition) {
        self.selection = Some(SelectionRange::new(anchor, head));
        self.mark_dirty();
    }

    pub fn clear_selection(&mut self) {
        if self.selection.is_some() {
            self.selection = None;
            self.mark_dirty();
        }
    }

    pub fn on_resize(&mut self, _cols: u16, rows: u16) {
        let usable = rows.saturating_sub(2) as usize;
        if usable != self.viewport_height {
            self.viewport_height = usable;
            if self.follow_tail {
                self.scroll_to_tail();
            }
            self.mark_dirty();
        }
    }

    pub fn mark_dirty(&mut self) {
        self.needs_redraw = true;
    }

    pub fn take_dirty(&mut self) -> bool {
        let was_dirty = self.needs_redraw;
        self.needs_redraw = false;
        was_dirty
    }

    pub fn render_frame(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let status_lines = if area.height >= 3 { 2 } else { 1 };
        let body_height = area.height.saturating_sub(status_lines as u16);
        self.viewport_height = body_height.max(1) as usize;
        if self.follow_tail {
            self.scroll_to_tail();
        } else {
            let max_scroll = self.rows.saturating_sub(self.viewport_height);
            self.scroll_top = self.scroll_top.min(max_scroll);
        }

        let mut constraints = vec![Constraint::Length(body_height)];
        for _ in 0..status_lines {
            constraints.push(Constraint::Length(1));
        }

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(area);

        if body_height > 0 {
            let body = self.render_body();
            frame.render_widget(body, chunks[0]);
        }

        if chunks.len() >= 2 {
            let status = self.render_status_line();
            frame.render_widget(status, chunks[1]);
        }

        if status_lines > 1 && chunks.len() >= 3 {
            let instructions = self.render_instructions();
            frame.render_widget(instructions, chunks[2]);
        }

        self.needs_redraw = false;
    }

    pub fn visible_lines(&self) -> Vec<String> {
        let height = self.viewport_height.max(1);
        let mut lines = Vec::with_capacity(height);
        for row_idx in 0..height {
            let absolute = self.scroll_top + row_idx;
            if absolute >= self.rows {
                lines.push(String::new());
                continue;
            }
            let mut line = String::with_capacity(self.cols.max(1));
            for col in 0..self.cols.max(1) {
                let (ch, _, _) = self.cell_for_render(absolute, col);
                line.push(ch);
            }
            lines.push(line);
        }
        lines
    }

    fn render_body(&self) -> Paragraph<'_> {
        let mut lines: Vec<Line> = Vec::with_capacity(self.viewport_height.max(1));
        for row_idx in 0..self.viewport_height.max(1) {
            let absolute = self.scroll_top + row_idx;
            if absolute >= self.rows {
                lines.push(Line::from(""));
                continue;
            }
            let mut spans = Vec::with_capacity(self.cols.max(1));
            for col in 0..self.cols.max(1) {
                let (ch, style_id, predicted) = self.cell_for_render(absolute, col);
                let selected = self
                    .selection
                    .as_ref()
                    .map(|sel| sel.contains(SelectionPosition { row: absolute, col }))
                    .unwrap_or(false);
                spans.push(self.span_for_cell(ch, style_id, selected, predicted));
            }
            lines.push(Line::from(spans));
        }

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::NONE))
            .scroll((0, 0))
            .alignment(Alignment::Left)
    }

    fn span_for_cell(
        &self,
        ch: char,
        style_id: Option<u32>,
        selected: bool,
        predicted: bool,
    ) -> Span<'static> {
        let mut style = match style_id {
            Some(_) => Style::default(),
            None => Style::default(),
        };
        if predicted {
            style = style
                .fg(Color::Yellow)
                .add_modifier(Modifier::ITALIC | Modifier::DIM);
        }
        if selected {
            style = style
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD);
        }
        Span::styled(ch.to_string(), style)
    }

    fn cell_for_render(&self, row: usize, col: usize) -> (char, Option<u32>, bool) {
        if let Some(predicted) = self.predictions.get(&(row, col)) {
            (predicted.ch, None, true)
        } else {
            let cell = self.cells[row][col];
            (cell.ch, cell.style_id, false)
        }
    }

    fn render_status_line(&self) -> Paragraph<'_> {
        let total_rows = self.rows;
        let displayed = self.viewport_height.min(total_rows);
        let follow = if self.follow_tail { "tail" } else { "manual" };
        let text = format!(
            "rows {total_rows} • showing {displayed} • scroll {} • mode {}",
            self.scroll_top, follow,
        );
        Paragraph::new(text).block(Block::default())
    }

    fn render_instructions(&self) -> Paragraph<'_> {
        let text = "alt+↑/↓ line • alt+PgUp/PgDn page • alt+End tail • alt+f follow";
        Paragraph::new(text).block(Block::default())
    }
}

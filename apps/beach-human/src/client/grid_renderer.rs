use crate::cache::Seq;
use crate::cache::terminal::StyleId;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Widget};
use std::collections::HashMap;

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
    pub row: u64,
    pub col: usize,
}

#[derive(Clone, Debug)]
struct SelectionRange {
    anchor: SelectionPosition,
    head: SelectionPosition,
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

#[derive(Clone, Copy, Debug)]
struct PredictedCell {
    ch: char,
    seq: Seq,
}

#[derive(Clone, Debug)]
struct CachedStyle {
    style: Style,
}

#[derive(Clone)]
struct RowState {
    cells: Vec<CellState>,
    latest_seq: Seq,
}

impl RowState {
    fn new(cols: usize) -> Self {
        Self {
            cells: vec![CellState::blank(); cols.max(1)],
            latest_seq: 0,
        }
    }

    fn ensure_cols(&mut self, cols: usize) {
        if self.cells.len() < cols {
            self.cells.resize(cols, CellState::blank());
        }
    }
}

#[derive(Clone)]
enum RowSlot {
    Pending,
    Loaded(RowState),
    Missing,
}

pub struct GridRenderer {
    base_row: u64,
    cols: usize,
    rows: Vec<RowSlot>,
    scroll_top: usize,
    viewport_height: usize,
    follow_tail: bool,
    selection: Option<SelectionRange>,
    needs_redraw: bool,
    predictions: HashMap<(u64, usize), PredictedCell>,
    status_message: Option<String>,
    styles: HashMap<u32, CachedStyle>,
}

impl GridRenderer {
    pub fn new(rows: usize, cols: usize) -> Self {
        let mut renderer = Self {
            base_row: 0,
            cols: cols.max(1),
            rows: Vec::new(),
            scroll_top: 0,
            viewport_height: 0,
            follow_tail: true,
            selection: None,
            needs_redraw: true,
            predictions: HashMap::new(),
            status_message: None,
            styles: HashMap::new(),
        };
        renderer.styles.insert(
            StyleId::DEFAULT.0,
            CachedStyle {
                style: Style::default(),
            },
        );
        renderer.ensure_capacity(rows, cols);
        renderer
    }

    pub fn base_row(&self) -> u64 {
        self.base_row
    }

    pub fn set_base_row(&mut self, base_row: u64) {
        if base_row == self.base_row {
            return;
        }
        if base_row > self.base_row {
            let drop = (base_row - self.base_row) as usize;
            let current_len = self.rows.len();
            if drop >= current_len {
                let preserve = current_len.max(self.viewport_height).max(1);
                self.rows.clear();
                self.rows.resize(preserve, RowSlot::Pending);
                self.scroll_top = 0;
            } else {
                self.rows.drain(0..drop);
                self.scroll_top = self.scroll_top.saturating_sub(drop);
            }
        } else {
            let add = (self.base_row - base_row) as usize;
            for _ in 0..add {
                self.rows.insert(0, RowSlot::Pending);
            }
            self.scroll_top = self.scroll_top.saturating_add(add);
        }
        self.base_row = base_row;
        self.mark_dirty();
    }

    fn ensure_capacity(&mut self, rows: usize, cols: usize) {
        if rows > self.rows.len() {
            for _ in self.rows.len()..rows {
                self.rows.push(RowSlot::Pending);
            }
        }
        if cols > self.cols {
            for slot in &mut self.rows {
                if let RowSlot::Loaded(state) = slot {
                    state.ensure_cols(cols);
                }
            }
            self.cols = cols;
        }
    }

    fn ensure_row(&mut self, absolute_row: u64) {
        let absolute = absolute_row;
        if absolute < self.base_row {
            return;
        }
        let required_len = (absolute - self.base_row) as usize + 1;
        if required_len > self.rows.len() {
            let missing = required_len - self.rows.len();
            for _ in 0..missing {
                self.rows.push(RowSlot::Pending);
            }
        }
    }

    pub fn ensure_size(&mut self, rows: usize, cols: usize) {
        self.ensure_capacity(rows, cols);
    }

    fn ensure_col(&mut self, col: usize) {
        if col < self.cols {
            return;
        }
        let new_cols = col + 1;
        for slot in &mut self.rows {
            if let RowSlot::Loaded(state) = slot {
                state.ensure_cols(new_cols);
            }
        }
        self.cols = new_cols;
    }

    fn relative_row(&self, absolute_row: u64) -> Option<usize> {
        let absolute = absolute_row;
        if absolute < self.base_row {
            None
        } else {
            let idx = (absolute - self.base_row) as usize;
            if idx < self.rows.len() {
                Some(idx)
            } else {
                None
            }
        }
    }

    fn touch_row(&mut self, absolute_row: u64) -> Option<usize> {
        let absolute = absolute_row;
        if absolute < self.base_row {
            let missing = (self.base_row - absolute) as usize;
            for _ in 0..missing {
                self.rows.insert(0, RowSlot::Pending);
            }
            self.base_row = absolute;
            self.scroll_top = self.scroll_top.saturating_add(missing);
            return Some(0);
        }
        self.ensure_row(absolute);
        Some((absolute - self.base_row) as usize)
    }

    fn row_state_mut(&mut self, rel: usize) -> Option<&mut RowState> {
        if rel >= self.rows.len() {
            return None;
        }
        if !matches!(self.rows[rel], RowSlot::Loaded(_)) {
            self.rows[rel] = RowSlot::Loaded(RowState::new(self.cols));
        }
        match &mut self.rows[rel] {
            RowSlot::Loaded(state) => {
                state.ensure_cols(self.cols);
                Some(state)
            }
            _ => None,
        }
    }

    pub fn apply_cell(
        &mut self,
        absolute_row: usize,
        col: usize,
        seq: Seq,
        ch: char,
        style_id: Option<u32>,
    ) {
        let absolute = absolute_row as u64;
        if let Some(rel) = self.touch_row(absolute) {
            self.ensure_col(col);
            self.clear_prediction_at(absolute_row, col);
            if let Some(state) = self.row_state_mut(rel) {
                let row = &mut state.cells;
                let cell = &mut row[col];
                if seq >= cell.seq {
                    cell.ch = ch;
                    cell.seq = seq;
                    cell.style_id = style_id;
                    state.latest_seq = state.latest_seq.max(seq);
                    self.mark_dirty();
                }
                if self.follow_tail {
                    self.scroll_to_tail();
                }
            }
        }
    }

    pub fn apply_row_from_text(&mut self, absolute_row: usize, seq: Seq, text: &str) {
        let absolute = absolute_row as u64;
        if self.touch_row(absolute).is_none() {
            return;
        }
        let width = text.chars().count();
        self.ensure_col(width);
        let total_cols = self.cols.max(1);
        let mut columns_to_clear: Vec<usize> = Vec::new();
        let mut changed = false;

        if let Some(rel) = self.relative_row(absolute) {
            {
                let state_opt = self.row_state_mut(rel);
                let state = match state_opt {
                    Some(state) => state,
                    None => return,
                };
                if seq < state.latest_seq {
                    return;
                }
                for (col, ch) in text.chars().enumerate() {
                    let cell = &mut state.cells[col];
                    if seq >= cell.seq {
                        cell.ch = ch;
                        cell.seq = seq;
                        cell.style_id = None;
                        changed = true;
                        columns_to_clear.push(col);
                    }
                }
                for col in width..total_cols {
                    let cell = &mut state.cells[col];
                    if seq >= cell.seq {
                        cell.ch = ' ';
                        cell.seq = seq;
                        cell.style_id = None;
                        changed = true;
                        columns_to_clear.push(col);
                    }
                }
                state.latest_seq = state.latest_seq.max(seq);
            }

            for col in columns_to_clear {
                self.clear_prediction_at(absolute_row, col);
            }

            if changed {
                self.mark_dirty();
            }
        }
    }

    pub fn apply_row_from_cells(
        &mut self,
        absolute_row: usize,
        seq: Seq,
        cells: &[(char, Option<u32>)],
    ) {
        let absolute = absolute_row as u64;
        if self.touch_row(absolute).is_none() {
            return;
        }
        self.ensure_col(cells.len());
        let total_cols = self.cols.max(1);
        let mut columns_to_clear: Vec<usize> = Vec::new();
        let mut changed = false;

        if let Some(rel) = self.relative_row(absolute) {
            {
                let state_opt = self.row_state_mut(rel);
                let state = match state_opt {
                    Some(state) => state,
                    None => return,
                };
                if seq < state.latest_seq {
                    return;
                }
                for (col, (ch, style_id)) in cells.iter().enumerate() {
                    let cell = &mut state.cells[col];
                    if seq >= cell.seq {
                        cell.ch = *ch;
                        cell.seq = seq;
                        cell.style_id = *style_id;
                        changed = true;
                        columns_to_clear.push(col);
                    }
                }
                for col in cells.len()..total_cols {
                    let cell = &mut state.cells[col];
                    if seq >= cell.seq {
                        cell.ch = ' ';
                        cell.seq = seq;
                        cell.style_id = None;
                        changed = true;
                        columns_to_clear.push(col);
                    }
                }
                state.latest_seq = state.latest_seq.max(seq);
            }

            for col in columns_to_clear {
                self.clear_prediction_at(absolute_row, col);
            }

            if changed {
                self.mark_dirty();
            }
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
        for absolute_row in rows.clone() {
            let absolute = absolute_row as u64;
            if let Some(rel) = self.touch_row(absolute) {
                self.ensure_col(cols.end);
                if let Some(state) = self.row_state_mut(rel) {
                    for col in cols.clone() {
                        let cell = &mut state.cells[col];
                        if seq >= cell.seq {
                            cell.ch = ch;
                            cell.seq = seq;
                            cell.style_id = style_id;
                        }
                    }
                    state.latest_seq = state.latest_seq.max(seq);
                    self.mark_dirty();
                }
            }
        }
        self.mark_dirty();
    }

    pub fn apply_segment(
        &mut self,
        absolute_row: usize,
        cells: &[(usize, Seq, char, Option<u32>)],
    ) {
        if cells.is_empty() {
            return;
        }
        for (col, seq, ch, style_id) in cells {
            self.apply_cell(absolute_row, *col, *seq, *ch, *style_id);
        }

        if let Some((first_col, _, _, _)) = cells.first() {
            if *first_col == 0 {
                let absolute = absolute_row as u64;
                if let Some(rel) = self.relative_row(absolute) {
                    if let Some(RowSlot::Loaded(state)) = self.rows.get_mut(rel) {
                        let last_seq = cells.last().map(|(_, seq, _, _)| *seq).unwrap_or(0);
                        let end_col = cells
                            .last()
                            .map(|(col, _, _, _)| col.saturating_add(1))
                            .unwrap_or(0);
                        if end_col < state.cells.len() {
                            for cell in state.cells.iter_mut().skip(end_col) {
                                if last_seq >= cell.seq {
                                    cell.ch = ' ';
                                    cell.seq = last_seq;
                                    cell.style_id = None;
                                }
                            }
                            state.latest_seq = state.latest_seq.max(last_seq);
                            self.mark_dirty();
                        }
                    }
                }
            }
        }
    }

    pub fn mark_row_missing(&mut self, absolute_row: u64) {
        if let Some(rel) = self.relative_row(absolute_row) {
            if !matches!(self.rows.get(rel), Some(RowSlot::Missing)) {
                self.rows[rel] = RowSlot::Missing;
                self.mark_dirty();
            }
        }
    }

    pub fn mark_row_pending(&mut self, absolute_row: u64) {
        if let Some(rel) = self.relative_row(absolute_row) {
            if !matches!(self.rows.get(rel), Some(RowSlot::Pending)) {
                self.rows[rel] = RowSlot::Pending;
                self.mark_dirty();
            }
        } else {
            self.touch_row(absolute_row);
            if let Some(rel) = self.relative_row(absolute_row) {
                self.rows[rel] = RowSlot::Pending;
                self.mark_dirty();
            }
        }
    }

    pub fn apply_trim(&mut self, start: usize, count: usize) {
        if count == 0 {
            return;
        }
        let start_abs = start as u64;
        let new_base = start_abs.saturating_add(count as u64);
        if new_base <= self.base_row {
            return;
        }
        let available = self.rows.len();
        let trim_count = (new_base - self.base_row).min(available as u64) as usize;
        if trim_count == 0 {
            return;
        }
        self.rows.drain(0..trim_count);
        self.base_row = self.base_row.saturating_add(trim_count as u64);
        self.scroll_top = self.scroll_top.saturating_sub(trim_count);
        if self.follow_tail {
            self.scroll_to_tail();
        }
        self.predictions.retain(|(row, _), _| *row >= self.base_row);
        if let Some(range) = &mut self.selection {
            let (anchor, head) = range.bounds();
            if anchor.row < self.base_row && head.row < self.base_row {
                self.clear_selection();
            } else {
                let new_anchor = SelectionPosition {
                    row: anchor.row.max(self.base_row),
                    col: anchor.col,
                };
                let new_head = SelectionPosition {
                    row: head.row.max(self.base_row),
                    col: head.col,
                };
                self.selection = Some(SelectionRange::new(new_anchor, new_head));
            }
        }
        self.mark_dirty();
    }

    pub fn add_prediction(&mut self, row: usize, col: usize, seq: Seq, ch: char) {
        self.predictions
            .insert((row as u64, col), PredictedCell { ch, seq });
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
        if self.predictions.remove(&(row as u64, col)).is_some() {
            self.mark_dirty();
        }
    }

    pub fn set_style(&mut self, id: u32, fg: u32, bg: u32, attrs: u8) {
        let style = decode_packed_style(fg, bg, attrs);
        self.styles.insert(id, CachedStyle { style });
        self.mark_dirty();
    }

    pub fn scroll_lines(&mut self, delta: isize) {
        if self.viewport_height == 0 {
            return;
        }
        let max_scroll = self.rows.len().saturating_sub(self.viewport_height);
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
        if self.viewport_height == 0 || self.rows.is_empty() {
            self.scroll_top = 0;
            return;
        }
        self.scroll_top = self.rows.len().saturating_sub(self.viewport_height);
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

    pub fn viewport_top(&self) -> u64 {
        self.base_row + self.scroll_top as u64
    }

    pub fn viewport_height(&self) -> usize {
        self.viewport_height.max(1)
    }

    pub fn total_rows(&self) -> u64 {
        self.base_row + self.rows.len() as u64
    }

    pub fn total_cols(&self) -> usize {
        self.cols
    }

    pub fn contains_row(&self, row: u64) -> bool {
        row >= self.base_row && row < self.base_row + self.rows.len() as u64
    }

    pub fn clamp_position(&self, row: i64, col: isize) -> SelectionPosition {
        if self.rows.is_empty() {
            return SelectionPosition {
                row: self.base_row,
                col: 0,
            };
        }
        let min_row = self.base_row.min(i64::MAX as u64) as i64;
        let max_row =
            (self.base_row + self.rows.len().saturating_sub(1) as u64).min(i64::MAX as u64) as i64;
        let clamped_row = row.clamp(min_row, max_row);
        let absolute_row = clamped_row as u64;
        let row_width = self.row_display_width(absolute_row);
        let max_col = if row_width == 0 {
            0
        } else {
            row_width.saturating_sub(1)
        } as isize;
        let clamped_col = col.clamp(0, max_col.max(0)) as usize;
        SelectionPosition {
            row: absolute_row,
            col: clamped_col,
        }
    }

    pub fn selection_text(&self) -> Option<String> {
        let range = self.selection.as_ref()?;
        if self.rows.is_empty() {
            return None;
        }
        let (start, end) = range.bounds();
        if end.row < self.base_row {
            return None;
        }
        let mut output = String::new();
        let mut current = start.row;
        while current <= end.row {
            if current < self.base_row {
                current += 1;
                continue;
            }
            if current >= self.base_row + self.rows.len() as u64 {
                break;
            }
            let mut row_start = if current == start.row { start.col } else { 0 };
            let mut row_end = if current == end.row {
                end.col
            } else {
                self.cols.saturating_sub(1)
            };
            row_start = row_start.min(self.cols.saturating_sub(1));
            row_end = row_end.min(self.cols.saturating_sub(1));
            if row_end >= row_start {
                for col in row_start..=row_end {
                    let (ch, _, _) = self.cell_for_render(current, col);
                    output.push(ch);
                }
            }
            if current != end.row {
                output.push('\n');
            }
            current += 1;
        }
        Some(output)
    }

    pub fn row_display_width(&self, absolute_row: u64) -> usize {
        if let Some(rel) = self.relative_row(absolute_row) {
            match &self.rows[rel] {
                RowSlot::Loaded(state) => state
                    .cells
                    .iter()
                    .rposition(|cell| cell.ch != ' ')
                    .map(|idx| idx + 1)
                    .unwrap_or(0),
                _ => 0,
            }
        } else {
            0
        }
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

    pub fn set_status_message<S: Into<String>>(&mut self, message: Option<S>) {
        self.status_message = message.map(Into::into);
        self.mark_dirty();
    }

    pub fn on_resize(&mut self, _cols: u16, rows: u16) {
        let usable = rows.saturating_sub(2) as usize;
        if usable != self.viewport_height {
            self.viewport_height = usable;
            if self.follow_tail {
                self.scroll_to_tail();
            } else {
                let max_scroll = self.rows.len().saturating_sub(self.viewport_height);
                self.scroll_top = self.scroll_top.min(max_scroll);
            }
            self.mark_dirty();
        }
    }

    fn has_pending_rows(&self) -> bool {
        self.rows
            .iter()
            .any(|slot| matches!(slot, RowSlot::Pending))
    }

    pub fn first_unloaded_range(&self, lookaround: usize) -> Option<(u64, u32)> {
        if self.rows.is_empty() {
            return None;
        }
        let first_visible = self.base_row + self.scroll_top as u64;
        let start = first_visible.saturating_sub(lookaround as u64);
        let mut span = self.viewport_height;
        span = span.saturating_add(lookaround);
        span = span.saturating_add(lookaround);
        let mut pending_start: Option<u64> = None;
        let mut count: u32 = 0;
        for offset in 0..=span {
            let absolute = start.saturating_add(offset as u64);
            if let Some(rel) = self.relative_row(absolute) {
                let loaded = matches!(
                    self.rows.get(rel),
                    Some(RowSlot::Loaded(_)) | Some(RowSlot::Missing)
                );
                if !loaded {
                    if pending_start.is_none() {
                        pending_start = Some(absolute);
                        count = 0;
                    }
                    count = count.saturating_add(1);
                } else if let Some(start_row) = pending_start {
                    return Some((start_row, count));
                }
            }
            if absolute == u64::MAX {
                break;
            }
        }
        pending_start.map(|row| (row, count))
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
            let max_scroll = self.rows.len().saturating_sub(self.viewport_height);
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
            let absolute = self.base_row + self.scroll_top as u64 + row_idx as u64;
            if let Some(rel) = self.relative_row(absolute) {
                match &self.rows[rel] {
                    RowSlot::Pending => {
                        lines.push("·".repeat(self.cols.max(1)));
                    }
                    RowSlot::Missing => {
                        lines.push(" ".repeat(self.cols.max(1)));
                    }
                    RowSlot::Loaded(_) => {
                        let mut line = String::with_capacity(self.cols.max(1));
                        for col in 0..self.cols.max(1) {
                            let (ch, _, _) = self.cell_for_render(absolute, col);
                            line.push(ch);
                        }
                        lines.push(line);
                    }
                }
            } else {
                lines.push(String::new());
            }
        }
        lines
    }

    fn render_body(&self) -> TerminalBodyWidget {
        let mut lines: Vec<Line<'static>> = Vec::with_capacity(self.viewport_height.max(1));
        for row_idx in 0..self.viewport_height.max(1) {
            let absolute = self.base_row + self.scroll_top as u64 + row_idx as u64;
            if let Some(rel) = self.relative_row(absolute) {
                match &self.rows[rel] {
                    RowSlot::Pending => {
                        let placeholder = "·".repeat(self.cols.max(1));
                        let style = Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::DIM);
                        lines.push(Line::from(vec![Span::styled(placeholder, style)]));
                    }
                    RowSlot::Missing => {
                        lines.push(Line::from(" ".repeat(self.cols.max(1))));
                    }
                    RowSlot::Loaded(_) => {
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
                }
            } else {
                lines.push(Line::from(" ".repeat(self.cols.max(1))));
            }
        }
        TerminalBodyWidget { lines }
    }

    fn span_for_cell(
        &self,
        ch: char,
        style_id: Option<u32>,
        selected: bool,
        predicted: bool,
    ) -> Span<'static> {
        let mut style = style_id
            .and_then(|id| self.styles.get(&id).map(|cached| cached.style.clone()))
            .unwrap_or_else(Style::default);
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

    fn cell_for_render(&self, absolute_row: u64, col: usize) -> (char, Option<u32>, bool) {
        if let Some(predicted) = self.predictions.get(&(absolute_row, col)) {
            (predicted.ch, None, true)
        } else if let Some(rel) = self.relative_row(absolute_row) {
            match &self.rows[rel] {
                RowSlot::Loaded(state) => {
                    if col < state.cells.len() {
                        let cell = state.cells[col];
                        (cell.ch, cell.style_id, false)
                    } else {
                        (' ', None, false)
                    }
                }
                RowSlot::Pending => ('·', None, false),
                RowSlot::Missing => (' ', None, false),
            }
        } else {
            (' ', None, false)
        }
    }

    #[cfg(test)]
    pub fn row_text_for_test(&self, absolute_row: u64) -> Option<String> {
        let rel = self.relative_row(absolute_row)?;
        match self.rows.get(rel)? {
            RowSlot::Loaded(state) => {
                let mut line = String::new();
                for cell in &state.cells {
                    line.push(cell.ch);
                }
                Some(line)
            }
            RowSlot::Pending => Some("·".repeat(self.cols)),
            RowSlot::Missing => Some(" ".repeat(self.cols)),
        }
    }

    fn render_status_line(&self) -> Paragraph<'_> {
        let total_rows = self.total_rows();
        let displayed = self.viewport_height.min(self.rows.len());
        let follow = if self.follow_tail { "tail" } else { "manual" };
        let status = self
            .status_message
            .as_deref()
            .unwrap_or("alt+[ copy • alt+f follow • alt+End tail");
        let loading = if self.has_pending_rows() {
            " • loading history"
        } else {
            ""
        };
        let text = format!(
            "rows {total_rows} • showing {displayed} • scroll {} • mode {} • {}{}",
            self.viewport_top(),
            follow,
            status,
            loading
        );
        Paragraph::new(text).block(Block::default())
    }

    fn render_instructions(&self) -> Paragraph<'_> {
        let text = "alt+↑/↓ line • alt+PgUp/PgDn page • alt+End tail • alt+f follow";
        Paragraph::new(text).block(Block::default())
    }
}

fn decode_packed_style(fg: u32, bg: u32, attrs: u8) -> Style {
    let mut style = Style::default();
    if let Some(color) = decode_color(fg) {
        style = style.fg(color);
    }
    if let Some(color) = decode_color(bg) {
        style = style.bg(color);
    }
    let modifiers = decode_modifiers(attrs);
    if !modifiers.is_empty() {
        style = style.add_modifier(modifiers);
    }
    style
}

fn decode_color(packed: u32) -> Option<Color> {
    match (packed >> 24) as u8 {
        0 => None,
        1 => Some(Color::Indexed((packed & 0xFF) as u8)),
        2 => Some(Color::Rgb(
            ((packed >> 16) & 0xFF) as u8,
            ((packed >> 8) & 0xFF) as u8,
            (packed & 0xFF) as u8,
        )),
        _ => None,
    }
}

fn decode_modifiers(attrs: u8) -> Modifier {
    let mut modifiers = Modifier::empty();
    if attrs & (1 << 0) != 0 {
        modifiers |= Modifier::BOLD;
    }
    if attrs & (1 << 1) != 0 {
        modifiers |= Modifier::ITALIC;
    }
    if attrs & (1 << 2) != 0 {
        modifiers |= Modifier::UNDERLINED;
    }
    if attrs & (1 << 3) != 0 {
        modifiers |= Modifier::CROSSED_OUT;
    }
    if attrs & (1 << 4) != 0 {
        modifiers |= Modifier::REVERSED;
    }
    if attrs & (1 << 5) != 0 {
        modifiers |= Modifier::SLOW_BLINK;
    }
    if attrs & (1 << 6) != 0 {
        modifiers |= Modifier::DIM;
    }
    if attrs & (1 << 7) != 0 {
        modifiers |= Modifier::HIDDEN;
    }
    modifiers
}

struct TerminalBodyWidget {
    lines: Vec<Line<'static>>,
}

impl Widget for TerminalBodyWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut blank_line: Option<Line<'static>> = None;
        let max_rows = area.height as usize;
        let lines = self.lines;
        for (idx, line) in lines.iter().enumerate().take(max_rows) {
            buf.set_line(area.x, area.y + idx as u16, line, area.width);
        }
        let rendered = lines.len().min(max_rows);
        for row in rendered..max_rows {
            let blank = blank_line.get_or_insert_with(|| {
                let repeated: String = std::iter::repeat(' ').take(area.width as usize).collect();
                Line::from(repeated)
            });
            buf.set_line(area.x, area.y + row as u16, blank, area.width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_line(text: &str) -> Vec<(char, Option<u32>)> {
        text.chars().map(|ch| (ch, None)).collect()
    }

    #[test]
    fn tail_render_does_not_leave_missing_gaps_after_new_rows() {
        let mut renderer = GridRenderer::new(0, 80);
        renderer.on_resize(80, 24);
        renderer.set_base_row(0);
        for row in 0..120u64 {
            renderer.mark_row_missing(row);
        }
        for (idx, row) in (121u64..151).enumerate() {
            let text = format!("Line {:03}", idx + 121);
            renderer.apply_row_from_cells(row as usize, row as Seq, &decode_line(&text));
        }
        renderer.scroll_to_tail();
        let lines = renderer.visible_lines();
        assert!(
            lines.iter().any(|line| line.contains("Line 150")),
            "tail missing expected content"
        );
        let non_blank = lines.iter().filter(|line| !line.trim().is_empty()).count();
        assert!(
            non_blank >= 24.min(lines.len()),
            "viewport still blank after filling rows"
        );
    }
}

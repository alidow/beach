use crate::cache::Seq;
use crate::cache::terminal::StyleId;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Widget};
use std::collections::HashMap;
use tracing::{Level, trace};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionMode {
    Character,
    Line,
    Block,
}

#[derive(Clone, Debug)]
struct SelectionRange {
    anchor: SelectionPosition,
    head: SelectionPosition,
    mode: SelectionMode,
}

#[derive(Clone, Copy, Debug)]
struct GridCursor {
    absolute_row: u64,
    col: usize,
    visible: bool,
}

impl SelectionRange {
    fn new(anchor: SelectionPosition, head: SelectionPosition, mode: SelectionMode) -> Self {
        Self { anchor, head, mode }
    }

    fn mode(&self) -> SelectionMode {
        self.mode
    }

    fn row_bounds(&self) -> (u64, u64) {
        let min = self.anchor.row.min(self.head.row);
        let max = self.anchor.row.max(self.head.row);
        (min, max)
    }

    fn ordered_positions(&self) -> (SelectionPosition, SelectionPosition) {
        if self.anchor.row < self.head.row
            || (self.anchor.row == self.head.row && self.anchor.col <= self.head.col)
        {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    fn block_col_bounds(&self) -> (usize, usize) {
        let min = self.anchor.col.min(self.head.col);
        let max = self.anchor.col.max(self.head.col);
        (min, max)
    }

    fn contains(&self, pos: SelectionPosition) -> bool {
        match self.mode {
            SelectionMode::Character => {
                let (start, end) = self.ordered_positions();
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
            SelectionMode::Line => {
                let (min_row, max_row) = self.row_bounds();
                pos.row >= min_row && pos.row <= max_row
            }
            SelectionMode::Block => {
                let (min_row, max_row) = self.row_bounds();
                if pos.row < min_row || pos.row > max_row {
                    return false;
                }
                let (min_col, max_col) = self.block_col_bounds();
                pos.col >= min_col && pos.col <= max_col
            }
        }
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

#[derive(Clone, Copy, Debug)]
struct GridUpdateDebugContext {
    frame: &'static str,
    update: &'static str,
    row: Option<u64>,
    seq: Option<Seq>,
}

#[derive(Clone)]
struct RowState {
    cells: Vec<CellState>,
    latest_seq: Seq,
    logical_width: usize,
}

impl RowState {
    fn new(cols: usize) -> Self {
        Self {
            cells: vec![CellState::blank(); cols.max(1)],
            latest_seq: 0,
            logical_width: 0,
        }
    }

    fn ensure_cols(&mut self, cols: usize) {
        if self.cells.len() < cols {
            self.cells.resize(cols, CellState::blank());
        }
    }

    fn is_blank(&self) -> bool {
        self.cells.iter().all(|cell| cell.ch == ' ')
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
    history_trimmed: bool,
    selection: Option<SelectionRange>,
    needs_redraw: bool,
    predictions: HashMap<(u64, usize), PredictedCell>,
    predictions_visible: bool,
    prediction_flagging: bool,
    status_message: Option<String>,
    status_highlight: Option<String>,
    status_is_error: bool,
    connection_status: Option<StatusIndicator>,
    styles: HashMap<u32, CachedStyle>,
    debug_context: Option<GridUpdateDebugContext>,
    cursor: Option<GridCursor>,
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
            history_trimmed: false,
            selection: None,
            needs_redraw: true,
            predictions: HashMap::new(),
            predictions_visible: false,
            prediction_flagging: false,
            status_message: None,
            status_highlight: None,
            status_is_error: false,
            connection_status: None,
            styles: HashMap::new(),
            debug_context: None,
            cursor: None,
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
                self.rows.clear();
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

    pub fn set_history_origin(&mut self, base_row: u64) {
        self.history_trimmed = base_row > 0;
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
            trace!(
                target = "client::render",
                absolute_row,
                rows = self.rows.len(),
                base_row = self.base_row,
                "ensure_row_extend"
            );
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
        let created = !matches!(self.rows[rel], RowSlot::Loaded(_));
        if created {
            self.rows[rel] = RowSlot::Loaded(RowState::new(self.cols));
            if tracing::enabled!(Level::TRACE) {
                let absolute_row = self.base_row + rel as u64;
                if let Some(ctx) = &self.debug_context {
                    trace!(
                        target = "client::cache",
                        frame = ctx.frame,
                        update = ctx.update,
                        row = ctx.row.unwrap_or(absolute_row),
                        seq = ctx.seq,
                        absolute_row,
                        base_row = self.base_row,
                        "created loaded row"
                    );
                } else {
                    trace!(
                        target = "client::cache",
                        absolute_row,
                        base_row = self.base_row,
                        "created loaded row"
                    );
                }
            }
        }
        match &mut self.rows[rel] {
            RowSlot::Loaded(state) => {
                state.ensure_cols(self.cols);
                Some(state)
            }
            _ => None,
        }
    }

    pub fn set_debug_update_context(
        &mut self,
        frame: &'static str,
        update: &'static str,
        row: Option<u64>,
        seq: Option<Seq>,
    ) {
        self.debug_context = Some(GridUpdateDebugContext {
            frame,
            update,
            row,
            seq,
        });
    }

    pub fn clear_debug_update_context(&mut self) {
        self.debug_context = None;
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
                    state.logical_width = state.logical_width.max(col.saturating_add(1));
                    self.mark_dirty();
                    trace!(
                        target = "client::render",
                        row = absolute_row,
                        col,
                        seq,
                        "apply_cell"
                    );
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
                let mut logical = 0usize;
                let mut all_spaces = true;
                for (col, ch) in text.chars().enumerate() {
                    let cell = &mut state.cells[col];
                    if seq >= cell.seq {
                        cell.ch = ch;
                        cell.seq = seq;
                        cell.style_id = None;
                        changed = true;
                        columns_to_clear.push(col);
                        trace!(
                            target = "client::render",
                            absolute_row, col, seq, "apply_row_from_text"
                        );
                    }
                    if ch != ' ' {
                        all_spaces = false;
                    }
                    logical = col + 1;
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
                state.logical_width = if all_spaces { 0 } else { logical };
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
                let mut logical = 0usize;
                let mut all_spaces = true;
                for (col, (ch, style_id)) in cells.iter().enumerate() {
                    let cell = &mut state.cells[col];
                    if seq >= cell.seq {
                        cell.ch = *ch;
                        cell.seq = seq;
                        cell.style_id = *style_id;
                        changed = true;
                        columns_to_clear.push(col);
                        trace!(
                            target = "client::render",
                            row = absolute_row,
                            col,
                            seq,
                            "apply_row_from_cells"
                        );
                    }
                    if *ch != ' ' {
                        all_spaces = false;
                    }
                    logical = col + 1;
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
                state.logical_width = if all_spaces { 0 } else { logical };
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
                let mut cleared_cols: Vec<usize> = Vec::new();
                let mut touched = false;
                if let Some(state) = self.row_state_mut(rel) {
                    if ch != ' ' {
                        state.logical_width = state.logical_width.max(cols.end);
                    } else if cols.start == 0 && cols.end >= state.logical_width {
                        state.logical_width = cols.start;
                    }
                    for col in cols.clone() {
                        let cell = &mut state.cells[col];
                        if seq >= cell.seq {
                            cell.ch = ch;
                            cell.seq = seq;
                            cell.style_id = style_id;
                            cleared_cols.push(col);
                            touched = true;
                            trace!(
                                target = "client::render",
                                row = absolute_row,
                                col,
                                seq,
                                "apply_rect"
                            );
                        }
                    }
                    state.latest_seq = state.latest_seq.max(seq);
                }
                if touched {
                    self.mark_dirty();
                }
                for col in cleared_cols {
                    self.clear_prediction_at(absolute_row, col);
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
            trace!(
                target = "client::render",
                row = absolute_row,
                col,
                seq,
                "apply_segment_cell"
            );
            self.apply_cell(absolute_row, *col, *seq, *ch, *style_id);
        }

        if let Some((first_col, _, _, _)) = cells.first() {
            if *first_col == 0 {
                let absolute = absolute_row as u64;
                if let Some(rel) = self.relative_row(absolute) {
                    let mut cleared_cols: Vec<usize> = Vec::new();
                    let mut touched = false;
                    if let Some(RowSlot::Loaded(state)) = self.rows.get_mut(rel) {
                        let last_seq = cells.last().map(|(_, seq, _, _)| *seq).unwrap_or(0);
                        let end_col = cells
                            .last()
                            .map(|(col, _, _, _)| col.saturating_add(1))
                            .unwrap_or(0);
                        if cells.iter().any(|(_, _, ch, _)| *ch != ' ') {
                            state.logical_width = end_col;
                        } else {
                            state.logical_width = 0;
                        }
                        if end_col < state.cells.len() {
                            for (offset, cell) in state.cells.iter_mut().enumerate().skip(end_col) {
                                if last_seq >= cell.seq {
                                    cell.ch = ' ';
                                    cell.seq = last_seq;
                                    cell.style_id = None;
                                    cleared_cols.push(offset);
                                    touched = true;
                                }
                            }
                            state.latest_seq = state.latest_seq.max(last_seq);
                        }
                    }
                    if touched {
                        self.mark_dirty();
                    }
                    for col in cleared_cols {
                        self.clear_prediction_at(absolute_row, col);
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
                trace!(
                    target = "client::render",
                    row = absolute_row,
                    "mark_row_missing"
                );
            }
        }
    }

    pub fn mark_row_pending(&mut self, absolute_row: u64) {
        if let Some(rel) = self.relative_row(absolute_row) {
            if !matches!(self.rows.get(rel), Some(RowSlot::Pending)) {
                self.rows[rel] = RowSlot::Pending;
                self.mark_dirty();
                trace!(
                    target = "client::render",
                    row = absolute_row,
                    "mark_row_pending"
                );
            }
        } else {
            self.touch_row(absolute_row);
            if let Some(rel) = self.relative_row(absolute_row) {
                self.rows[rel] = RowSlot::Pending;
                self.mark_dirty();
                trace!(
                    target = "client::render",
                    row = absolute_row,
                    "mark_row_pending_touch"
                );
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
        if new_base > 0 {
            self.history_trimmed = true;
        }
        self.predictions.retain(|(row, _), _| *row >= self.base_row);
        if let Some(mut range) = self.selection.take() {
            let (_, max_row) = range.row_bounds();
            if max_row < self.base_row {
                self.mark_dirty();
            } else {
                if range.anchor.row < self.base_row {
                    range.anchor.row = self.base_row;
                }
                if range.head.row < self.base_row {
                    range.head.row = self.base_row;
                }
                self.selection = Some(range);
                self.mark_dirty();
            }
        }
        self.mark_dirty();
    }

    pub fn add_prediction(&mut self, row: usize, col: usize, seq: Seq, ch: char) {
        self.predictions
            .insert((row as u64, col), PredictedCell { ch, seq });
        self.mark_dirty();
    }

    pub fn set_predictions_visible(&mut self, visible: bool) {
        if self.predictions_visible != visible {
            self.predictions_visible = visible;
            self.mark_dirty();
        }
    }

    pub fn set_prediction_flagging(&mut self, underline: bool) {
        if self.prediction_flagging != underline {
            self.prediction_flagging = underline;
            self.mark_dirty();
        }
    }

    pub fn has_active_predictions(&self) -> bool {
        !self.predictions.is_empty()
    }

    pub fn prediction_exists(&self, row: usize, col: usize, seq: Seq) -> bool {
        self.predictions
            .get(&(row as u64, col))
            .is_some_and(|cell| cell.seq == seq)
    }

    pub fn seq_has_predictions(&self, seq: Seq) -> bool {
        self.predictions.values().any(|cell| cell.seq == seq)
    }

    pub fn cell_matches(&self, row: usize, col: usize, ch: char) -> bool {
        let absolute = row as u64;
        if let Some(rel) = self.relative_row(absolute) {
            if let Some(RowSlot::Loaded(state)) = self.rows.get(rel) {
                if col < state.cells.len() {
                    return state.cells[col].ch == ch;
                }
            }
        }
        ch == ' '
    }

    fn logical_row_width(&self, absolute_row: u64) -> usize {
        self.relative_row(absolute_row)
            .and_then(|rel| match self.rows.get(rel) {
                Some(RowSlot::Loaded(state)) => Some(state.logical_width),
                _ => None,
            })
            .unwrap_or(0)
    }

    pub fn committed_row_width(&self, absolute_row: u64) -> usize {
        self.logical_row_width(absolute_row)
            .max(self.row_display_width(absolute_row))
    }

    pub fn predicted_row_width(&self, absolute_row: u64) -> usize {
        self.predictions
            .iter()
            .filter(|((row, _), _)| *row == absolute_row)
            .map(|((_, col), _)| col.saturating_add(1))
            .max()
            .unwrap_or(0)
    }

    pub fn effective_row_width(&self, absolute_row: u64) -> usize {
        let committed = self
            .logical_row_width(absolute_row)
            .max(self.row_display_width(absolute_row));
        let predicted = self.predicted_row_width(absolute_row);
        committed.max(predicted)
    }

    pub fn shrink_row_to_column(&mut self, absolute_row: u64, col: usize) -> bool {
        let mut changed = false;
        if let Some(rel) = self.relative_row(absolute_row) {
            if let Some(RowSlot::Loaded(state)) = self.rows.get_mut(rel) {
                if state.logical_width > col {
                    let mut new_width = state.logical_width;
                    while new_width > col {
                        if new_width == 0 {
                            break;
                        }
                        let idx = new_width - 1;
                        if idx >= state.cells.len() {
                            new_width = new_width.saturating_sub(1);
                            continue;
                        }
                        if state.cells[idx].ch != ' ' {
                            break;
                        }
                        new_width = new_width.saturating_sub(1);
                    }
                    if new_width < state.logical_width {
                        state.logical_width = new_width;
                        changed = true;
                    }
                }
            }
        }

        let before = self.predictions.len();
        self.predictions.retain(|(row, prediction_col), _| {
            if *row == absolute_row && *prediction_col >= col {
                changed = true;
                false
            } else {
                true
            }
        });
        if changed || self.predictions.len() != before {
            self.mark_dirty();
            true
        } else {
            false
        }
    }

    pub fn clear_prediction_seq(&mut self, seq: Seq) {
        let before = self.predictions.len();
        self.predictions.retain(|_, cell| cell.seq != seq);
        if self.predictions.len() != before {
            self.mark_dirty();
        }
    }

    pub fn shift_predictions_left(&mut self, row: usize, delta: usize) -> bool {
        if delta == 0 {
            return false;
        }
        let row_key = row as u64;
        let mut moved: Vec<((u64, usize), PredictedCell)> = Vec::new();
        let mut to_remove: Vec<(u64, usize)> = Vec::new();
        for (&(pred_row, col), cell) in self.predictions.iter() {
            if pred_row == row_key {
                let new_col = col.saturating_sub(delta);
                to_remove.push((pred_row, col));
                moved.push(((pred_row, new_col), *cell));
            }
        }
        if to_remove.is_empty() {
            return false;
        }
        for key in to_remove {
            self.predictions.remove(&key);
        }
        for (key, cell) in moved {
            self.predictions.insert(key, cell);
        }
        self.mark_dirty();
        true
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

    pub fn ensure_row_visible(&mut self, absolute_row: u64) {
        if self.viewport_height == 0 {
            return;
        }
        if absolute_row < self.base_row {
            return;
        }
        let rel = match self.relative_row(absolute_row) {
            Some(rel) => rel,
            None => return,
        };
        let viewport = self.viewport_height.max(1);
        if rel < self.scroll_top {
            self.scroll_top = rel;
            self.follow_tail = false;
            self.mark_dirty();
        } else if rel >= self.scroll_top.saturating_add(viewport) {
            let desired = rel.saturating_sub(viewport.saturating_sub(1));
            let max_scroll = self.rows.len().saturating_sub(viewport);
            self.scroll_top = desired.min(max_scroll);
            self.follow_tail = false;
            self.mark_dirty();
        }
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
        let max_scroll = self.rows.len().saturating_sub(self.viewport_height);
        let target = self
            .last_loaded_row_index()
            .map(|idx| idx.saturating_sub(self.viewport_height.saturating_sub(1)))
            .unwrap_or(max_scroll);
        self.scroll_top = target.min(max_scroll);
        self.follow_tail = true;
        self.mark_dirty();
        trace!(
            target = "client::render",
            base_row = self.base_row,
            scroll_top = self.scroll_top,
            viewport_height = self.viewport_height,
            rows = self.rows.len(),
            "scroll_to_tail"
        );
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
        trace!(
            target = "client::render",
            follow_tail = self.follow_tail,
            base_row = self.base_row,
            scroll_top = self.scroll_top,
            "set_follow_tail"
        );
    }

    pub fn toggle_follow_tail(&mut self) {
        let follow = !self.follow_tail;
        self.set_follow_tail(follow);
    }

    pub fn is_following_tail(&self) -> bool {
        self.follow_tail
    }

    fn last_loaded_row_index(&self) -> Option<usize> {
        let mut last_loaded: Option<usize> = None;
        for (idx, slot) in self.rows.iter().enumerate().rev() {
            if let RowSlot::Loaded(state) = slot {
                if !state.is_blank() {
                    return Some(idx);
                }
                if last_loaded.is_none() {
                    last_loaded = Some(idx);
                }
            }
        }
        last_loaded
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
        match range.mode() {
            SelectionMode::Character => {
                let (start, end) = range.ordered_positions();
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
            SelectionMode::Line => {
                let (min_row, max_row) = range.row_bounds();
                if max_row < self.base_row {
                    return None;
                }
                let mut output = String::new();
                let mut row = min_row;
                while row <= max_row {
                    if row < self.base_row {
                        row += 1;
                        continue;
                    }
                    if row >= self.base_row + self.rows.len() as u64 {
                        break;
                    }
                    let width = self.row_display_width(row);
                    if width == 0 {
                        // Preserve blank line for empty row selections
                    } else {
                        for col in 0..width {
                            let (ch, _, _) = self.cell_for_render(row, col);
                            output.push(ch);
                        }
                    }
                    if row != max_row {
                        output.push('\n');
                    }
                    row += 1;
                }
                Some(output)
            }
            SelectionMode::Block => {
                let (min_row, max_row) = range.row_bounds();
                if max_row < self.base_row {
                    return None;
                }
                let (mut min_col, mut max_col) = range.block_col_bounds();
                if self.cols == 0 {
                    return Some(String::new());
                }
                let max_valid_col = self.cols.saturating_sub(1);
                min_col = min_col.min(max_valid_col);
                max_col = max_col.min(max_valid_col);
                if max_col < min_col {
                    return Some(String::new());
                }
                let mut output = String::new();
                let mut row = min_row;
                while row <= max_row {
                    if row < self.base_row {
                        row += 1;
                        continue;
                    }
                    if row >= self.base_row + self.rows.len() as u64 {
                        break;
                    }
                    for col in min_col..=max_col {
                        let (ch, _, _) = self.cell_for_render(row, col);
                        output.push(ch);
                    }
                    if row != max_row {
                        output.push('\n');
                    }
                    row += 1;
                }
                Some(output)
            }
        }
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

    pub fn set_selection(
        &mut self,
        anchor: SelectionPosition,
        head: SelectionPosition,
        mode: SelectionMode,
    ) {
        self.selection = Some(SelectionRange::new(anchor, head, mode));
        self.mark_dirty();
    }

    pub fn clear_selection(&mut self) {
        if self.selection.is_some() {
            self.selection = None;
            self.mark_dirty();
        }
    }

    pub fn set_status_message<S: Into<String>>(&mut self, message: Option<S>) {
        self.set_status_internal(message.map(Into::into), None, false);
    }

    pub fn set_status_error_message<S: Into<String>>(&mut self, message: Option<S>) {
        self.set_status_internal(message.map(Into::into), None, true);
    }

    pub fn set_status_with_highlight<S1, S2>(&mut self, message: Option<S1>, highlight: Option<S2>)
    where
        S1: Into<String>,
        S2: Into<String>,
    {
        self.set_status_internal(message.map(Into::into), highlight.map(Into::into), false);
    }

    fn set_status_internal(
        &mut self,
        message: Option<String>,
        highlight: Option<String>,
        error: bool,
    ) {
        self.status_message = message;
        if error {
            self.status_highlight = None;
        } else {
            self.status_highlight = highlight;
        }
        self.status_is_error = error && self.status_message.is_some();
        self.mark_dirty();
    }

    #[cfg(test)]
    pub fn status_for_test(&self) -> (Option<String>, bool) {
        (self.status_message.clone(), self.status_is_error)
    }

    pub fn set_connection_status<S: Into<String>>(&mut self, text: S, style: Style) {
        self.connection_status = Some(StatusIndicator {
            text: text.into(),
            style,
        });
        self.mark_dirty();
    }

    pub fn clear_connection_status(&mut self) {
        if self.connection_status.is_some() {
            self.connection_status = None;
            self.mark_dirty();
        }
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

    pub fn has_pending_rows(&self) -> bool {
        self.rows
            .iter()
            .any(|slot| matches!(slot, RowSlot::Pending))
    }

    pub fn has_missing_rows(&self) -> bool {
        self.rows
            .iter()
            .any(|slot| matches!(slot, RowSlot::Missing))
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
                let loaded = matches!(self.rows.get(rel), Some(RowSlot::Loaded(_)));
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

    pub fn set_cursor(&mut self, absolute_row: u64, col: usize, visible: bool) {
        self.cursor = Some(GridCursor {
            absolute_row,
            col,
            visible,
        });
        self.mark_dirty();
    }

    pub fn clear_cursor(&mut self) {
        if self.cursor.is_some() {
            self.cursor = None;
            self.mark_dirty();
        }
    }

    pub fn get_cursor(&self) -> Option<(u64, usize, bool)> {
        self.cursor
            .as_ref()
            .map(|c| (c.absolute_row, c.col, c.visible))
    }

    fn cursor_viewport_offset(&self) -> Option<(usize, usize)> {
        let cursor = self.cursor?;
        if !cursor.visible {
            return None;
        }
        let viewport_start = self.base_row + self.scroll_top as u64;
        if cursor.absolute_row < viewport_start {
            return None;
        }
        let row_offset = cursor.absolute_row - viewport_start;
        if row_offset >= self.viewport_height as u64 {
            return None;
        }
        let col = if self.cols == 0 {
            0
        } else {
            cursor.col.min(self.cols.saturating_sub(1))
        };
        Some((col, row_offset as usize))
    }

    pub fn cursor_viewport_position(&self) -> Option<(u16, u16)> {
        let (col, row) = self.cursor_viewport_offset()?;
        Some((col as u16, row as u16))
    }

    pub fn cursor_widget_position(&self, area: Rect) -> Option<(u16, u16)> {
        let (col, row) = self.cursor_viewport_offset()?;
        if col as u16 >= area.width || row as u16 >= area.height {
            return None;
        }
        Some((area.x + col as u16, area.y + row as u16))
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
            if let Some((cursor_x, cursor_y)) = self.cursor_widget_position(chunks[0]) {
                frame.set_cursor_position((cursor_x, cursor_y));
            }
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
        let mut entries: Vec<(String, bool, u64)> = Vec::with_capacity(height);
        for row_idx in 0..height {
            let absolute = self.base_row + self.scroll_top as u64 + row_idx as u64;
            if let Some(rel) = self.relative_row(absolute) {
                match &self.rows[rel] {
                    RowSlot::Pending => {
                        entries.push(("·".repeat(self.cols.max(1)), true, absolute))
                    }
                    RowSlot::Missing => {
                        entries.push((" ".repeat(self.cols.max(1)), true, absolute))
                    }
                    RowSlot::Loaded(_) => {
                        let mut line = String::with_capacity(self.cols.max(1));
                        for col in 0..self.cols.max(1) {
                            let (ch, _, _) = self.cell_for_render(absolute, col);
                            line.push(ch);
                        }
                        entries.push((line, false, absolute));
                    }
                }
            } else {
                entries.push((String::new(), true, absolute));
            }
        }
        if self.follow_tail && (self.base_row > 0 || self.scroll_top > 0 || self.history_trimmed) {
            let mut trimmed_absolute: Option<u64> = None;
            let mut trimmed = 0usize;
            while let Some((_, is_pending, abs)) = entries.last() {
                if *is_pending {
                    trimmed_absolute = Some(*abs);
                    entries.pop();
                    trimmed += 1;
                } else {
                    break;
                }
            }
            if trimmed > 0 {
                trace!(
                    target = "client::tail",
                    event = "trim_pending_suffix",
                    trimmed,
                    last_trimmed = trimmed_absolute,
                    base_row = self.base_row,
                    scroll_top = self.scroll_top,
                    viewport = self.viewport_height,
                    rows = self.rows.len()
                );
            }
        }
        let sample_rows: Vec<_> = entries
            .iter()
            .take(5)
            .map(|(_, is_pending, abs)| format!("{}:{}", abs, if *is_pending { 'P' } else { 'L' }))
            .collect();
        let mut lines = Vec::with_capacity(height);
        let blanks_needed = height.saturating_sub(entries.len());
        for _ in 0..blanks_needed {
            lines.push(" ".repeat(self.cols.max(1)));
        }
        lines.extend(entries.into_iter().map(|(line, _, _)| line));
        lines.truncate(height);
        if tracing::enabled!(Level::TRACE) {
            if let Some(last) = lines.last() {
                trace!(
                    target = "client::tail",
                    event = "visible_lines",
                    base_row = self.base_row,
                    scroll_top = self.scroll_top,
                    viewport = self.viewport_height,
                    rows = self.rows.len(),
                    follow = self.follow_tail,
                    blanks = blanks_needed,
                    sample_rows = ?sample_rows,
                    last_line = %last.trim()
                );
            }
        }
        lines
    }

    fn render_body(&self) -> TerminalBodyWidget {
        let height = self.viewport_height.max(1);
        let mut entries: Vec<(Line<'static>, bool, u64)> = Vec::with_capacity(height);
        for row_idx in 0..height {
            let absolute = self.base_row + self.scroll_top as u64 + row_idx as u64;
            if let Some(rel) = self.relative_row(absolute) {
                match &self.rows[rel] {
                    RowSlot::Pending => {
                        let placeholder = "·".repeat(self.cols.max(1));
                        let style = Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::DIM);
                        entries.push((
                            Line::from(vec![Span::styled(placeholder, style)]),
                            true,
                            absolute,
                        ));
                    }
                    RowSlot::Missing => {
                        entries.push((Line::from(" ".repeat(self.cols.max(1))), true, absolute))
                    }
                    RowSlot::Loaded(_) => {
                        let mut spans = Vec::with_capacity(self.cols.max(1));
                        let cursor_col_for_row = self
                            .cursor
                            .filter(|cursor| cursor.visible && cursor.absolute_row == absolute)
                            .map(|cursor| {
                                if self.cols == 0 {
                                    0
                                } else {
                                    cursor.col.min(self.cols.saturating_sub(1))
                                }
                            });
                        for col in 0..self.cols.max(1) {
                            let (ch, style_id, predicted) = self.cell_for_render(absolute, col);
                            let selected = self
                                .selection
                                .as_ref()
                                .map(|sel| sel.contains(SelectionPosition { row: absolute, col }))
                                .unwrap_or(false);
                            let highlight_cursor = cursor_col_for_row
                                .map(|cursor_col| cursor_col == col)
                                .unwrap_or(false);
                            spans.push(self.span_for_cell(
                                ch,
                                style_id,
                                selected,
                                predicted,
                                highlight_cursor,
                            ));
                        }
                        entries.push((Line::from(spans), false, absolute));
                    }
                }
            } else {
                entries.push((Line::from(" ".repeat(self.cols.max(1))), true, absolute));
            }
        }
        if self.follow_tail && (self.base_row > 0 || self.scroll_top > 0 || self.history_trimmed) {
            let mut trimmed = 0usize;
            while let Some((_, is_pending, _)) = entries.last() {
                if *is_pending {
                    entries.pop();
                    trimmed += 1;
                } else {
                    break;
                }
            }
            if trimmed > 0 {
                trace!(
                    target = "client::tail",
                    event = "render_body_trim",
                    trimmed,
                    base_row = self.base_row,
                    scroll_top = self.scroll_top,
                    viewport = self.viewport_height,
                    rows = self.rows.len()
                );
            }
        }
        let sample_rows: Vec<_> = entries
            .iter()
            .take(5)
            .map(|(_, is_pending, abs)| format!("{}:{}", abs, if *is_pending { 'P' } else { 'L' }))
            .collect();
        let mut lines: Vec<Line<'static>> = Vec::with_capacity(height);
        let blanks_needed = height.saturating_sub(entries.len());
        for _ in 0..blanks_needed {
            lines.push(Line::from(" ".repeat(self.cols.max(1))));
        }
        lines.extend(entries.into_iter().map(|(line, _, _)| line));
        lines.truncate(height);
        if tracing::enabled!(Level::TRACE) {
            if let Some(last) = lines.last() {
                let rendered = last
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>();
                trace!(
                    target = "client::tail",
                    event = "render_body",
                    base_row = self.base_row,
                    scroll_top = self.scroll_top,
                    viewport = self.viewport_height,
                    rows = self.rows.len(),
                    blanks = blanks_needed,
                    sample_rows = ?sample_rows,
                    last_line = %rendered.trim()
                );
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
        highlight_cursor: bool,
    ) -> Span<'static> {
        let mut style = style_id
            .and_then(|id| self.styles.get(&id).map(|cached| cached.style))
            .unwrap_or_default();
        if predicted && self.prediction_flagging {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        if selected {
            style = style
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD);
        }
        if highlight_cursor {
            style = style.add_modifier(Modifier::REVERSED | Modifier::BOLD);
        }
        Span::styled(ch.to_string(), style)
    }

    fn cell_for_render(&self, absolute_row: u64, col: usize) -> (char, Option<u32>, bool) {
        if self.predictions_visible {
            if let Some(predicted) = self.predictions.get(&(absolute_row, col)) {
                return (predicted.ch, None, true);
            }
        }
        if let Some(rel) = self.relative_row(absolute_row) {
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

    pub fn row_text(&self, absolute_row: u64) -> Option<String> {
        let rel = self.relative_row(absolute_row)?;
        match self.rows.get(rel)? {
            RowSlot::Loaded(state) => {
                let mut line = String::new();
                for cell in &state.cells {
                    line.push(cell.ch);
                }
                Some(line)
            }
            RowSlot::Pending | RowSlot::Missing => None,
        }
    }

    #[cfg(test)]
    pub fn row_text_for_test(&self, absolute_row: u64) -> Option<String> {
        if let Some(line) = self.row_text(absolute_row) {
            return Some(line);
        }
        let rel = self.relative_row(absolute_row)?;
        match self.rows.get(rel)? {
            RowSlot::Pending => Some("·".repeat(self.cols)),
            RowSlot::Missing => Some(" ".repeat(self.cols)),
            _ => None,
        }
    }

    pub fn first_gap_between(&self, start: u64, end: u64) -> Option<(u64, u32)> {
        if start >= end {
            return None;
        }
        let mut gap_start: Option<u64> = None;
        let mut gap_len: u32 = 0;
        let mut absolute = start;
        while absolute < end {
            if absolute < self.base_row {
                absolute = self.base_row;
            }
            if absolute >= self.base_row + self.rows.len() as u64 {
                break;
            }
            let rel = (absolute - self.base_row) as usize;
            let loaded = matches!(
                self.rows.get(rel),
                Some(RowSlot::Loaded(_)) | Some(RowSlot::Missing)
            );
            if !loaded {
                if gap_start.is_none() {
                    gap_start = Some(absolute);
                    gap_len = 0;
                }
                gap_len = gap_len.saturating_add(1);
            } else if let Some(start_row) = gap_start {
                return Some((start_row, gap_len));
            }
            absolute = absolute.saturating_add(1);
        }
        gap_start.map(|row| (row, gap_len))
    }

    fn render_status_line(&self) -> Paragraph<'_> {
        let total_rows = self.total_rows();
        let displayed = self.viewport_height.min(self.rows.len());
        let status_line = format!(
            "rows {total_rows} • showing {displayed} • scroll {}",
            self.viewport_top()
        );
        let mut spans: Vec<Span> = Vec::new();
        if let Some(indicator) = &self.connection_status {
            spans.push(Span::styled(indicator.text.clone(), indicator.style));
            spans.push(Span::raw("  "));
        }
        spans.push(Span::raw(status_line));

        if let Some(message) = &self.status_message {
            spans.push(Span::raw(format!(" • {}", message)));
        }

        if !self.status_is_error {
            if let Some(highlight) = &self.status_highlight {
                spans.push(Span::styled(
                    format!(" • {}", highlight),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ));
            }
        }

        Paragraph::new(Line::from(spans)).block(Block::default())
    }

    fn render_instructions(&self) -> Paragraph<'_> {
        let text = "CTRL+Q quit";
        Paragraph::new(text).block(Block::default())
    }
}

struct StatusIndicator {
    text: String,
    style: Style,
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
                let repeated = " ".repeat(area.width as usize);
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

    #[test_timeout::timeout]
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

    #[test_timeout::timeout]
    fn tail_short_buffer_stays_top_aligned() {
        let mut renderer = GridRenderer::new(0, 10);
        renderer.on_resize(10, 8); // viewport_height reduced by status lines
        renderer.set_base_row(0);
        renderer.set_history_origin(0);
        for row in 0..3u64 {
            let text = format!("Row {row}");
            renderer.apply_row_from_cells(row as usize, row as Seq, &decode_line(&text));
        }
        renderer.scroll_to_tail();
        renderer.viewport_height = 6;
        let lines = renderer.visible_lines();
        assert_eq!(lines.len(), 6);
        assert!(lines[0].starts_with("Row 0"));
    }

    #[test_timeout::timeout]
    fn tail_short_buffer_bottom_aligns_when_history_trimmed() {
        let mut renderer = GridRenderer::new(0, 10);
        renderer.on_resize(10, 8);
        renderer.set_base_row(120);
        renderer.set_history_origin(120);
        for idx in 0..3u64 {
            let abs = 120 + idx;
            let text = format!("Row {abs}");
            renderer.apply_row_from_cells(abs as usize, abs as Seq, &decode_line(&text));
        }
        renderer.scroll_to_tail();
        renderer.viewport_height = 6;
        let lines = renderer.visible_lines();
        assert_eq!(lines.len(), 6);
        assert_eq!(lines.last().unwrap().trim(), "Row 122");
    }
}

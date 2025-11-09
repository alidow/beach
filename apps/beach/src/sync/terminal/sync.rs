use std::cmp;
use std::collections::{HashSet, VecDeque};
use std::convert::TryFrom;
use std::sync::Arc;

use crate::cache::terminal::{PackedCell, TerminalGrid, unpack_cell};
use crate::cache::{GridCache, Seq};
use crate::model::terminal::diff::{CacheUpdate, HistoryTrim, RowSnapshot, StyleDefinition};
use crate::sync::{
    DeltaSlice, DeltaSource, PriorityLane, SnapshotSlice, SnapshotSource, SyncConfig, SyncUpdate,
    Watermark,
};

pub trait TerminalDeltaStream: Send + Sync {
    fn collect_since(&self, since: Seq, budget: usize) -> Vec<CacheUpdate>;
    fn latest_seq(&self) -> Seq;
}

pub struct NullTerminalDeltaStream;

impl TerminalDeltaStream for NullTerminalDeltaStream {
    fn collect_since(&self, _since: Seq, _budget: usize) -> Vec<CacheUpdate> {
        Vec::new()
    }
    fn latest_seq(&self) -> Seq {
        0
    }
}

pub struct TerminalSync {
    grid: Arc<TerminalGrid>,
    delta_stream: Arc<dyn TerminalDeltaStream>,
    config: SyncConfig,
}

impl TerminalSync {
    pub fn new(
        grid: Arc<TerminalGrid>,
        delta_stream: Arc<dyn TerminalDeltaStream>,
        config: SyncConfig,
    ) -> Self {
        Self {
            grid,
            delta_stream,
            config,
        }
    }

    fn collect_row(
        &self,
        row_index: usize,
        cursor: &mut TerminalSnapshotCursor,
    ) -> Option<CacheUpdate> {
        let (_, cols) = self.grid.dims();
        let absolute_row = self.grid.row_id_at(row_index)?;
        let absolute_row_usize = usize::try_from(absolute_row).ok()?;
        if cols == 0 {
            return Some(CacheUpdate::Row(RowSnapshot::new(
                absolute_row_usize,
                0,
                Vec::new(),
            )));
        }
        let mut cells = Vec::with_capacity(cols);
        let mut max_seq = 0;
        for col in 0..cols {
            let snapshot = self.grid.get_cell_relaxed(row_index, col)?;
            max_seq = max_seq.max(snapshot.seq);
            cells.push(snapshot.cell);
        }
        self.enqueue_styles_for_cells(&cells, max_seq, cursor);
        Some(CacheUpdate::Row(RowSnapshot::new(
            absolute_row_usize,
            max_seq,
            cells,
        )))
    }

    fn collect_row_by_absolute(
        &self,
        absolute_row: u64,
        cursor: &mut TerminalSnapshotCursor,
    ) -> Option<CacheUpdate> {
        let index = self.grid.index_of_row(absolute_row)?;
        self.collect_row(index, cursor)
    }

    fn initial_snapshot_floor(&self, first: u64, last: u64) -> u64 {
        let initial = self.config.initial_snapshot_lines;
        if initial == 0 {
            last.checked_add(1).unwrap_or(u64::MAX)
        } else {
            let span = (initial.saturating_sub(1)) as u64;
            let candidate = last.saturating_sub(span);
            candidate.max(first)
        }
    }

    fn max_seq_from_grid(&self) -> Seq {
        let (rows, cols) = self.grid.dims();
        let mut max_seq = 0;
        for row in 0..rows {
            for col in 0..cols {
                if let Some(snapshot) = self.grid.get_cell_relaxed(row, col) {
                    max_seq = max_seq.max(snapshot.seq);
                }
            }
        }
        max_seq
    }

    fn seed_style_table(&self, cursor: &mut TerminalSnapshotCursor) {
        for (style_id, style) in self.grid.style_table.entries() {
            let id = style_id.0;
            if id == 0 {
                continue;
            }
            if cursor.emitted_styles.insert(id) {
                cursor
                    .pending_styles
                    .push_back(CacheUpdate::Style(StyleDefinition::new(style_id, 0, style)));
            }
        }
        cursor.styles_seeded = true;
    }

    fn enqueue_styles_for_cells(
        &self,
        cells: &[PackedCell],
        seq: Seq,
        cursor: &mut TerminalSnapshotCursor,
    ) {
        for packed in cells {
            let (_, style_id) = unpack_cell(*packed);
            let id = style_id.0;
            if id == 0 {
                continue;
            }
            if cursor.emitted_styles.insert(id) {
                if let Some(style) = self.grid.style_table.get(style_id) {
                    cursor
                        .pending_styles
                        .push_back(CacheUpdate::Style(StyleDefinition::new(
                            style_id, seq, style,
                        )));
                }
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct TerminalSnapshotCursor {
    next_foreground_row: Option<u64>,
    foreground_floor: u64,
    next_recent_row: Option<u64>,
    recent_floor: u64,
    next_history_row: Option<u64>,
    pending_styles: VecDeque<CacheUpdate>,
    pending_rows: VecDeque<CacheUpdate>,
    emitted_styles: HashSet<u32>,
    styles_seeded: bool,
}

impl SnapshotSource<CacheUpdate> for TerminalSync {
    type Cursor = TerminalSnapshotCursor;

    fn max_seq(&self) -> Seq {
        cmp::max(self.max_seq_from_grid(), self.delta_stream.latest_seq())
    }

    fn reset_lane(&self, cursor: &mut Self::Cursor, lane: PriorityLane) {
        let first_row_id = self.grid.first_row_id();
        let last_row_id = self.grid.last_row_id();
        match lane {
            PriorityLane::Foreground => {
                cursor.pending_styles.clear();
                cursor.pending_rows.clear();
                cursor.emitted_styles.clear();
                cursor.styles_seeded = false;
                if let Some(last) = last_row_id {
                    let first = first_row_id.unwrap_or(last);
                    let floor = self.initial_snapshot_floor(first, last);
                    cursor.foreground_floor = floor;
                    cursor.next_foreground_row = if floor <= last { Some(last) } else { None };
                } else {
                    cursor.foreground_floor = 0;
                    cursor.next_foreground_row = None;
                }
            }
            PriorityLane::Recent => {
                if let (Some(first), Some(last)) = (first_row_id, last_row_id) {
                    cursor.recent_floor = first;
                    let floor = self.initial_snapshot_floor(first, last);
                    cursor.next_recent_row = if floor > first {
                        floor.checked_sub(1)
                    } else {
                        None
                    };
                } else {
                    cursor.recent_floor = 0;
                    cursor.next_recent_row = None;
                }
            }
            PriorityLane::History => {
                cursor.next_history_row = None;
            }
        }
    }

    fn next_slice(
        &self,
        cursor: &mut Self::Cursor,
        lane: PriorityLane,
        budget: usize,
    ) -> Option<SnapshotSlice<CacheUpdate>> {
        if budget == 0 {
            return None;
        }
        let mut updates = Vec::new();
        let mut max_seq = 0;

        match lane {
            PriorityLane::Foreground => {
                if !cursor.styles_seeded {
                    self.seed_style_table(cursor);
                }
                while updates.len() < budget {
                    if let Some(style_update) = cursor.pending_styles.pop_front() {
                        max_seq = max_seq.max(style_update.seq());
                        updates.push(style_update);
                        continue;
                    }
                    if let Some(row_update) = cursor.pending_rows.pop_front() {
                        max_seq = max_seq.max(row_update.seq());
                        updates.push(row_update);
                        continue;
                    }
                    let absolute = match cursor.next_foreground_row {
                        Some(row) => row,
                        None => break,
                    };
                    if absolute < cursor.foreground_floor {
                        cursor.next_foreground_row = None;
                        break;
                    }
                    let next_value = if absolute == cursor.foreground_floor {
                        None
                    } else {
                        absolute.checked_sub(1)
                    };
                    cursor.next_foreground_row = next_value;
                    if let Some(update) = self.collect_row_by_absolute(absolute, cursor) {
                        cursor.pending_rows.push_back(update);
                        continue;
                    }
                    if cursor.next_foreground_row.is_none() {
                        break;
                    }
                }
            }
            PriorityLane::Recent => {
                while updates.len() < budget {
                    if let Some(style_update) = cursor.pending_styles.pop_front() {
                        max_seq = max_seq.max(style_update.seq());
                        updates.push(style_update);
                        continue;
                    }
                    if let Some(row_update) = cursor.pending_rows.pop_front() {
                        max_seq = max_seq.max(row_update.seq());
                        updates.push(row_update);
                        continue;
                    }
                    let absolute = match cursor.next_recent_row {
                        Some(row) => row,
                        None => break,
                    };
                    if absolute < cursor.recent_floor {
                        cursor.next_recent_row = None;
                        break;
                    }
                    let reached_floor = absolute == cursor.recent_floor;
                    let next_value = if reached_floor {
                        None
                    } else {
                        absolute.checked_sub(1)
                    };
                    cursor.next_recent_row = next_value;
                    if let Some(update) = self.collect_row_by_absolute(absolute, cursor) {
                        cursor.pending_rows.push_back(update);
                        continue;
                    }
                    if cursor.next_recent_row.is_none() {
                        break;
                    }
                }
            }
            PriorityLane::History => {
                while updates.len() < budget {
                    if let Some(style_update) = cursor.pending_styles.pop_front() {
                        max_seq = max_seq.max(style_update.seq());
                        updates.push(style_update);
                        continue;
                    }
                    if let Some(row_update) = cursor.pending_rows.pop_front() {
                        max_seq = max_seq.max(row_update.seq());
                        updates.push(row_update);
                        continue;
                    }
                    break;
                }
                if updates.is_empty() {
                    return None;
                }
            }
        }

        if updates.is_empty() {
            return None;
        }

        let has_more = match lane {
            PriorityLane::Foreground => cursor.next_foreground_row.is_some(),
            PriorityLane::Recent => cursor.next_recent_row.is_some(),
            PriorityLane::History => cursor.next_history_row.is_some(),
        };

        Some(SnapshotSlice {
            updates,
            watermark: Watermark(max_seq),
            has_more,
        })
    }
}

impl DeltaSource<CacheUpdate> for TerminalSync {
    fn next_delta(&self, since: Seq, budget: usize) -> Option<DeltaSlice<CacheUpdate>> {
        if budget == 0 {
            return None;
        }
        let mut updates = Vec::new();
        let mut remaining = budget;

        let trim_events = self.grid.drain_trim_events();
        if !trim_events.is_empty() {
            let mut total = 0usize;
            let mut start = trim_events.first().map(|e| e.start).unwrap_or(0) as usize;
            if let Some(first) = trim_events.first() {
                start = first.start as usize;
            }
            for event in trim_events {
                total += event.count;
            }
            if total > 0 {
                updates.push(CacheUpdate::Trim(HistoryTrim::new(start, total)));
                remaining = remaining.saturating_sub(1);
            }
        }

        if remaining == 0 {
            let watermark = updates.iter().map(|u| u.seq()).max().unwrap_or(since);
            return Some(DeltaSlice {
                updates,
                watermark: Watermark(watermark),
                has_more: true,
            });
        }

        let delta_updates = self.delta_stream.collect_since(since, remaining);
        let has_more_delta =
            delta_updates.len() == remaining && self.delta_stream.latest_seq() > since;
        updates.extend(delta_updates);
        if updates.is_empty() {
            return None;
        }
        let watermark = updates.iter().map(|u| u.seq()).max().unwrap_or(since);
        let has_more = has_more_delta || updates.len() == budget;
        Some(DeltaSlice {
            updates,
            watermark: Watermark(watermark),
            has_more,
        })
    }
}

impl SyncUpdate for CacheUpdate {
    fn seq(&self) -> Seq {
        self.seq()
    }

    fn cost(&self) -> usize {
        match self {
            CacheUpdate::Cell(_) => 1,
            CacheUpdate::Rect(rect) => rect.area(),
            CacheUpdate::Row(row) => row.width(),
            CacheUpdate::Trim(_) => 1,
            CacheUpdate::Style(_) => 1,
            CacheUpdate::Cursor(_) => 1,
        }
    }
}

impl TerminalSync {
    pub fn config(&self) -> &SyncConfig {
        &self.config
    }
    pub fn grid(&self) -> &Arc<TerminalGrid> {
        &self.grid
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::terminal::packed::attrs_to_byte;
    use crate::cache::terminal::{
        Style, StyleId, TerminalGrid, pack_color_from_heavy, unpack_cell,
    };
    use crate::model::terminal::cell::{CellAttributes, Color as HeavyColor};
    use crate::model::terminal::{CellWrite, RectFill};
    use crate::sync::{LaneBudget, ServerSynchronizer, SubscriptionId};

    #[derive(Default)]
    struct MockDeltaStream {
        timeline: Vec<CacheUpdate>,
        latest_seq: Seq,
    }

    impl MockDeltaStream {
        fn new(timeline: Vec<CacheUpdate>) -> Self {
            let latest_seq = timeline.iter().map(|u| u.seq()).max().unwrap_or(0);
            Self {
                timeline,
                latest_seq,
            }
        }
    }

    impl TerminalDeltaStream for MockDeltaStream {
        fn collect_since(&self, since: Seq, budget: usize) -> Vec<CacheUpdate> {
            self.timeline
                .iter()
                .filter(|update| update.seq() > since)
                .take(budget)
                .cloned()
                .collect()
        }

        fn latest_seq(&self) -> Seq {
            self.latest_seq
        }
    }

    fn fill_row(grid: &TerminalGrid, row: usize, text: &str, seq_start: Seq) {
        let style = Style::default();
        let style_id = grid.ensure_style_id(style);
        for (col, ch) in text.chars().enumerate() {
            let packed = TerminalGrid::pack_char_with_style(ch, style_id);
            let seq = seq_start + col as Seq;
            grid.write_packed_cell_if_newer(row, col, seq, packed)
                .unwrap();
        }
    }

    #[test_timeout::timeout]
    fn snapshot_and_delta_streaming() {
        let rows = 200;
        let cols = 10;
        let grid = Arc::new(TerminalGrid::new(rows, cols));

        for row in 0..rows {
            let text = format!("line-{row:03}");
            fill_row(&grid, row, &text, (row as Seq) * 1000);
        }

        let delta_updates = vec![
            CacheUpdate::Cell(CellWrite::new(
                199,
                0,
                300000,
                TerminalGrid::pack_char_with_style('X', StyleId::DEFAULT),
            )),
            CacheUpdate::Rect(RectFill::new(
                198..200,
                0..2,
                300010,
                TerminalGrid::pack_char_with_style('Y', StyleId::DEFAULT),
            )),
        ];
        let delta_stream = Arc::new(MockDeltaStream::new(delta_updates));

        let config = SyncConfig {
            snapshot_budgets: vec![
                LaneBudget::new(PriorityLane::Foreground, 5),
                LaneBudget::new(PriorityLane::Recent, 8),
                LaneBudget::new(PriorityLane::History, 12),
            ],
            delta_budget: 2,
            initial_snapshot_lines: 5,
            ..Default::default()
        };

        let terminal_sync = Arc::new(TerminalSync::new(
            grid.clone(),
            delta_stream.clone(),
            config.clone(),
        ));
        let mut server_sync = ServerSynchronizer::new(terminal_sync.clone(), config.clone());

        let subscription_id = SubscriptionId(42);
        let hello = server_sync.hello(subscription_id);
        assert_eq!(hello.subscription_id, subscription_id);

        let mut foreground_total = 0;
        let mut recent_rows = 0;
        let mut history_rows = 0;
        let mut last_foreground_watermark = 0;

        while let Some(chunk) =
            server_sync.snapshot_chunk(subscription_id, PriorityLane::Foreground)
        {
            assert_eq!(chunk.subscription_id, subscription_id);
            for update in &chunk.updates {
                match update {
                    CacheUpdate::Row(row) => {
                        foreground_total += 1;
                        last_foreground_watermark = last_foreground_watermark.max(row.seq);
                    }
                    CacheUpdate::Style(_) => {}
                    _ => panic!("foreground lane expected row or style snapshots"),
                }
            }
            assert!(chunk.updates.len() <= config.budget_for(PriorityLane::Foreground));
            if !chunk.has_more {
                break;
            }
        }
        assert_eq!(foreground_total, config.initial_snapshot_lines.min(rows));

        while let Some(chunk) = server_sync.snapshot_chunk(subscription_id, PriorityLane::Recent) {
            for update in &chunk.updates {
                match update {
                    CacheUpdate::Row(row) => {
                        assert!(row.row < rows);
                        recent_rows += 1;
                    }
                    CacheUpdate::Style(_) => {}
                    _ => panic!("recent lane expected row snapshots"),
                }
            }
            assert!(chunk.updates.len() <= config.budget_for(PriorityLane::Recent));
            if !chunk.has_more {
                break;
            }
        }
        assert_eq!(recent_rows, rows - foreground_total);

        while let Some(chunk) = server_sync.snapshot_chunk(subscription_id, PriorityLane::History) {
            assert!(chunk.updates.is_empty());
            history_rows += chunk.updates.len();
        }
        assert_eq!(history_rows, 0);

        let since = last_foreground_watermark;
        if let Some(delata_batch) = server_sync.delta_batch(subscription_id, since) {
            assert!(delata_batch.updates.len() <= config.delta_budget);
            assert!(
                delata_batch
                    .updates
                    .iter()
                    .all(|update| update.seq() > since)
            );
        }
    }

    #[test_timeout::timeout]
    fn snapshot_emits_styles_before_rows() {
        let grid = Arc::new(TerminalGrid::new(4, 4));
        let style = Style {
            fg: pack_color_from_heavy(&HeavyColor::Rgb(255, 0, 0)),
            bg: pack_color_from_heavy(&HeavyColor::Default),
            attrs: attrs_to_byte(&CellAttributes {
                bold: true,
                ..CellAttributes::default()
            }),
        };
        let style_id = grid.ensure_style_id(style);
        let packed = TerminalGrid::pack_char_with_style('Z', style_id);
        grid.write_packed_cell_if_newer(0, 0, 1, packed).unwrap();
        let snapshot = grid.get_cell_relaxed(0, 0).unwrap();
        assert_eq!(unpack_cell(snapshot.cell).1.0, style_id.0);

        let delta_stream = Arc::new(NullTerminalDeltaStream);
        let config = SyncConfig {
            snapshot_budgets: vec![
                LaneBudget::new(PriorityLane::Foreground, 4),
                LaneBudget::new(PriorityLane::Recent, 4),
                LaneBudget::new(PriorityLane::History, 4),
            ],
            initial_snapshot_lines: 1,
            ..Default::default()
        };
        let terminal_sync = Arc::new(TerminalSync::new(grid, delta_stream, config.clone()));
        let mut server_sync = ServerSynchronizer::new(terminal_sync, config);
        let subscription_id = SubscriptionId(7);
        let chunk = server_sync
            .snapshot_chunk(subscription_id, PriorityLane::Foreground)
            .expect("expected snapshot chunk");
        assert!(matches!(chunk.updates.first(), Some(CacheUpdate::Style(_))));
        assert!(
            chunk
                .updates
                .iter()
                .any(|update| matches!(update, CacheUpdate::Row(_)))
        );
    }
}

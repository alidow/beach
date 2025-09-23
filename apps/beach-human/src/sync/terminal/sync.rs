use std::cmp;
use std::convert::TryFrom;
use std::sync::Arc;

use crate::cache::terminal::TerminalGrid;
use crate::cache::{GridCache, Seq};
use crate::model::terminal::diff::{CacheUpdate, HistoryTrim, RowSnapshot};
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

    fn collect_row(&self, row_index: usize) -> Option<CacheUpdate> {
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
        Some(CacheUpdate::Row(RowSnapshot::new(
            absolute_row_usize,
            max_seq,
            cells,
        )))
    }

    fn collect_row_by_absolute(&self, absolute_row: u64) -> Option<CacheUpdate> {
        let index = self.grid.index_of_row(absolute_row)?;
        self.collect_row(index)
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
}

#[derive(Debug, Default)]
pub struct TerminalSnapshotCursor {
    next_foreground_row: Option<u64>,
    foreground_floor: u64,
    next_recent_row: Option<u64>,
    recent_floor: u64,
    next_history_row: Option<u64>,
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
                while updates.len() < budget {
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
                    if let Some(update) = self.collect_row_by_absolute(absolute) {
                        max_seq = max_seq.max(update.seq());
                        updates.push(update);
                    }
                    cursor.next_foreground_row = next_value;
                    if cursor.next_foreground_row.is_none() {
                        break;
                    }
                }
            }
            PriorityLane::Recent => {
                while updates.len() < budget {
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
                    if let Some(update) = self.collect_row_by_absolute(absolute) {
                        max_seq = max_seq.max(update.seq());
                        updates.push(update);
                    }
                    cursor.next_recent_row = next_value;
                    if cursor.next_recent_row.is_none() {
                        break;
                    }
                }
            }
            PriorityLane::History => {
                return None;
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
                if remaining > 0 {
                    remaining -= 1;
                }
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
    use crate::cache::terminal::{Style, StyleId, TerminalGrid};
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

    #[test]
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

        let mut config = SyncConfig::default();
        config.snapshot_budgets = vec![
            LaneBudget::new(PriorityLane::Foreground, 5),
            LaneBudget::new(PriorityLane::Recent, 8),
            LaneBudget::new(PriorityLane::History, 12),
        ];
        config.delta_budget = 2;
        config.initial_snapshot_lines = 5;

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
        let mut foreground_first_chunk = None;
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
                    _ => panic!("foreground lane expected row snapshots"),
                }
            }
            if foreground_first_chunk.is_none() {
                foreground_first_chunk = Some(chunk.updates.len());
            }
            assert!(chunk.updates.len() <= config.budget_for(PriorityLane::Foreground));
            if !chunk.has_more {
                break;
            }
        }
        assert_eq!(foreground_total, config.initial_snapshot_lines.min(rows));
        assert_eq!(
            foreground_first_chunk.unwrap_or(0),
            config.budget_for(PriorityLane::Foreground)
        );

        while let Some(chunk) = server_sync.snapshot_chunk(subscription_id, PriorityLane::Recent) {
            for update in &chunk.updates {
                match update {
                    CacheUpdate::Row(row) => {
                        assert!(row.row < rows);
                        recent_rows += 1;
                    }
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
}

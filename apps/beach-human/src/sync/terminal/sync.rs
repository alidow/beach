use std::cmp;
use std::sync::Arc;

use crate::cache::terminal::TerminalGrid;
use crate::cache::{GridCache, Seq};
use crate::model::terminal::diff::{CacheUpdate, RowSnapshot};
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

    fn collect_row(&self, row: usize) -> Option<CacheUpdate> {
        let (_, cols) = self.grid.dims();
        if cols == 0 {
            return Some(CacheUpdate::Row(RowSnapshot::new(row, 0, Vec::new())));
        }
        let mut cells = Vec::with_capacity(cols);
        let mut max_seq = 0;
        for col in 0..cols {
            let snapshot = self.grid.get_cell_relaxed(row, col)?;
            max_seq = max_seq.max(snapshot.seq);
            cells.push(snapshot.cell);
        }
        Some(CacheUpdate::Row(RowSnapshot::new(row, max_seq, cells)))
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
    next_foreground_row: Option<isize>,
    next_recent_row: Option<isize>,
    next_history_row: usize,
    history_progress: usize,
}

impl SnapshotSource<CacheUpdate> for TerminalSync {
    type Cursor = TerminalSnapshotCursor;

    fn max_seq(&self) -> Seq {
        cmp::max(self.max_seq_from_grid(), self.delta_stream.latest_seq())
    }

    fn reset_lane(&self, cursor: &mut Self::Cursor, lane: PriorityLane) {
        let (rows, _) = self.grid.dims();
        match lane {
            PriorityLane::Foreground => {
                cursor.next_foreground_row = rows.checked_sub(1).map(|r| r as isize);
            }
            PriorityLane::Recent => {
                let viewport_rows = self.config.budget_for(PriorityLane::Foreground).min(rows);
                let start = rows.saturating_sub(viewport_rows).saturating_sub(1);
                cursor.next_recent_row = if rows <= viewport_rows {
                    None
                } else {
                    Some(start as isize)
                };
            }
            PriorityLane::History => {
                cursor.next_history_row = 0;
                cursor.history_progress = 0;
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
        let (rows, _) = self.grid.dims();
        if rows == 0 {
            return None;
        }
        let mut updates = Vec::new();
        let mut max_seq = 0;

        match lane {
            PriorityLane::Foreground => {
                let mut row = cursor.next_foreground_row?;
                for _ in 0..budget {
                    if row < 0 {
                        break;
                    }
                    if let Some(update) = self.collect_row(row as usize) {
                        max_seq = max_seq.max(update.seq());
                        updates.push(update);
                    }
                    row -= 1;
                }
                cursor.next_foreground_row = if row < 0 { None } else { Some(row) };
            }
            PriorityLane::Recent => {
                let mut row = cursor.next_recent_row?;
                let floor = cursor.history_progress as isize;
                for _ in 0..budget {
                    if row < floor {
                        break;
                    }
                    if let Some(update) = self.collect_row(row as usize) {
                        max_seq = max_seq.max(update.seq());
                        updates.push(update);
                    }
                    row -= 1;
                }
                cursor.next_recent_row = if row < floor { None } else { Some(row) };
            }
            PriorityLane::History => {
                let mut row = cursor.next_history_row;
                for _ in 0..budget {
                    if row >= rows {
                        break;
                    }
                    if let Some(update) = self.collect_row(row) {
                        max_seq = max_seq.max(update.seq());
                        updates.push(update);
                    }
                    row += 1;
                }
                cursor.next_history_row = row;
                cursor.history_progress = row;
            }
        }

        if updates.is_empty() {
            return None;
        }

        let has_more = match lane {
            PriorityLane::Foreground => cursor.next_foreground_row.is_some(),
            PriorityLane::Recent => cursor.next_recent_row.is_some(),
            PriorityLane::History => cursor.next_history_row < rows,
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
        let updates = self.delta_stream.collect_since(since, budget);
        if updates.is_empty() {
            return None;
        }
        let watermark = updates.iter().map(|u| u.seq()).max().unwrap_or(since);
        let has_more = updates.len() == budget && self.delta_stream.latest_seq() > watermark;
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
        assert!(foreground_total >= config.budget_for(PriorityLane::Foreground));
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
        assert!(recent_rows > 0);

        let mut history_seen = 0;
        while let Some(chunk) = server_sync.snapshot_chunk(subscription_id, PriorityLane::History) {
            if chunk.updates.is_empty() {
                break;
            }
            for update in &chunk.updates {
                if let CacheUpdate::Row(row) = update {
                    history_rows += 1;
                    history_seen = history_seen.max(row.row + 1);
                }
            }
            assert!(chunk.updates.len() <= config.budget_for(PriorityLane::History));
            if !chunk.has_more {
                break;
            }
        }
        assert_eq!(history_seen, rows);
        assert!(history_rows >= rows);

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

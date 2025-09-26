mod emulator;
mod pty;

pub use emulator::{AlacrittyEmulator, EmulatorResult, SimpleTerminalEmulator, TerminalEmulator};
pub use pty::{Command, PtyProcess, PtyReader, PtyWriter, SpawnConfig, resize_pty};

use crate::cache::terminal::{PackedCell, TerminalGrid, unpack_cell};
use crate::model::terminal::diff::CacheUpdate;
use crate::telemetry::{self, PerfGuard};
use anyhow::Result;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tracing::{self, Level, trace};

#[derive(Debug, Default)]
pub struct LocalEcho {
    expected: Mutex<VecDeque<u8>>,
}

impl LocalEcho {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_input(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let mut guard = self.expected.lock().unwrap();
        guard.extend(bytes.iter().copied());
    }

    pub fn consume_echo_prefix(&self, chunk: &[u8]) -> usize {
        if chunk.is_empty() {
            return 0;
        }
        let mut guard = self.expected.lock().unwrap();
        let mut consumed = 0;
        while consumed < chunk.len() {
            let Some(expected) = guard.front().copied() else {
                break;
            };
            if expected == chunk[consumed] {
                guard.pop_front();
                consumed += 1;
            } else {
                guard.clear();
                break;
            }
        }
        consumed
    }

    pub fn clear(&self) {
        self.expected.lock().unwrap().clear();
    }
}

pub struct TerminalRuntime {
    process: Arc<PtyProcess>,
    writer: PtyWriter,
    reader_handle: JoinHandle<()>,
    emulator: Arc<Mutex<Box<dyn TerminalEmulator + Send>>>,
}

impl TerminalRuntime {
    pub fn spawn(
        config: SpawnConfig,
        emulator: Box<dyn TerminalEmulator + Send>,
        grid: Arc<TerminalGrid>,
        mirror_stdout: bool,
        local_echo: Option<Arc<LocalEcho>>,
    ) -> Result<(Self, UnboundedReceiver<CacheUpdate>)> {
        let (process_raw, reader, writer) = PtyProcess::spawn(config)?;
        let process = Arc::new(process_raw);
        let emulator = Arc::new(Mutex::new(emulator));
        let (tx, rx) = mpsc::unbounded_channel();

        let reader_handle = tokio::spawn(read_loop(
            reader,
            emulator.clone(),
            grid,
            tx,
            mirror_stdout,
            local_echo.clone(),
        ));

        Ok((
            Self {
                process,
                writer,
                reader_handle,
                emulator,
            },
            rx,
        ))
    }

    pub fn writer(&self) -> PtyWriter {
        self.writer.clone()
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.process.resize(cols, rows)?;
        if let Ok(mut emulator) = self.emulator.lock() {
            emulator.resize(rows as usize, cols as usize);
        }
        Ok(())
    }

    pub fn process_handle(&self) -> Arc<PtyProcess> {
        self.process.clone()
    }

    pub fn emulator_handle(&self) -> Arc<Mutex<Box<dyn TerminalEmulator + Send>>> {
        self.emulator.clone()
    }

    pub fn shutdown(&self) {
        self.process.shutdown();
    }

    pub async fn wait(self) -> Result<()> {
        let TerminalRuntime {
            process,
            reader_handle,
            ..
        } = self;

        let _ = reader_handle.await;
        process.wait().await
    }
}

async fn read_loop(
    reader: PtyReader,
    emulator: Arc<Mutex<Box<dyn TerminalEmulator + Send>>>,
    grid: Arc<TerminalGrid>,
    tx: UnboundedSender<CacheUpdate>,
    mirror_stdout: bool,
    local_echo: Option<Arc<LocalEcho>>,
) {
    use std::io::Write;

    loop {
        match reader.read_chunk().await {
            Ok(Some(chunk)) => {
                let skip = local_echo
                    .as_ref()
                    .map(|echo| echo.consume_echo_prefix(&chunk))
                    .unwrap_or(0);
                let forwarded = if skip >= chunk.len() {
                    &[][..]
                } else {
                    &chunk[skip..]
                };
                telemetry::record_bytes("pty_read_bytes", chunk.len());
                let _guard = PerfGuard::new("pty_chunk_process");
                if mirror_stdout {
                    if let Err(err) = std::io::stdout().write_all(&chunk) {
                        eprintln!("⚠️  stdout mirror failed: {err}");
                    } else {
                        let _ = std::io::stdout().flush();
                    }
                } else if forwarded.is_empty() {
                    continue;
                }
                let updates = {
                    let mut emulator = emulator.lock().unwrap();
                    emulator.handle_output(&chunk, &grid)
                };
                for update in updates {
                    log_update_sample(&update);
                    apply_update(&grid, &update);
                    let _ = tx.send(update);
                }
            }
            Ok(None) => {
                if let Some(echo) = &local_echo {
                    echo.clear();
                }
                let _guard = PerfGuard::new("pty_flush");
                let flushes = {
                    let mut emulator = emulator.lock().unwrap();
                    emulator.flush(&grid)
                };
                for update in flushes {
                    log_update_sample(&update);
                    apply_update(&grid, &update);
                    let _ = tx.send(update);
                }
                break;
            }
            Err(err) => {
                if let Some(echo) = &local_echo {
                    echo.clear();
                }
                eprintln!("pty reader error: {err:?}");
                break;
            }
        }
    }
}

fn log_update_sample(update: &CacheUpdate) {
    if !tracing::enabled!(Level::TRACE) {
        return;
    }
    match update {
        CacheUpdate::Row(row) => {
            let text: String = row.cells.iter().map(|cell| unpack_cell(*cell).0).collect();
            let trimmed = text.trim_end();
            if !trimmed.is_empty() {
                trace!(target = "server::grid", row = row.row, seq = row.seq, sample = %trimmed);
            }
        }
        CacheUpdate::Cell(cell) => {
            let (ch, _) = unpack_cell(cell.cell.into());
            if ch != ' ' {
                trace!(target = "server::grid", row = cell.row, col = cell.col, seq = cell.seq, ch = %ch);
            }
        }
        _ => {}
    }
}

fn apply_update(grid: &TerminalGrid, update: &CacheUpdate) {
    match update {
        CacheUpdate::Cell(cell) => {
            let _ = grid.write_packed_cell_if_newer(cell.row, cell.col, cell.seq, cell.cell);
        }
        CacheUpdate::Rect(rect) => {
            let _ = grid.fill_rect_with_cell_if_newer(
                rect.rows.start,
                rect.cols.start,
                rect.rows.end,
                rect.cols.end,
                rect.seq,
                rect.cell,
            );
        }
        CacheUpdate::Row(row) => {
            if tracing::enabled!(Level::TRACE) {
                let text: String = row
                    .cells
                    .iter()
                    .map(|cell| unpack_cell(PackedCell::from(*cell)).0)
                    .collect();
                if text.contains("Line ") {
                    trace!(
                        target = "server::grid",
                        row = row.row,
                        seq = row.seq,
                        sample = %text.trim_end_matches(' ')
                    );
                }
            }
            for (offset, cell) in row.cells.iter().enumerate() {
                let _ = grid.write_packed_cell_if_newer(row.row, offset, row.seq, *cell);
            }
        }
        CacheUpdate::Trim(_) => {
            // grid already applied trim when emitting the event
        }
        CacheUpdate::Style(style) => {
            let _ = grid.style_table.set(style.id, style.style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::terminal::{PackedCell, TerminalGrid, unpack_cell};
    use tokio::time::{Duration, sleep};

    #[test_timeout::tokio_timeout_test]
    async fn runtime_captures_command_output() {
        let grid = Arc::new(TerminalGrid::new(4, 20));
        let emulator: Box<dyn TerminalEmulator + Send> = Box::new(AlacrittyEmulator::new(&grid));
        let command = Command::new("/usr/bin/env").arg("printf").arg("hello");
        let config = SpawnConfig::new(command, 80, 24);

        let spawn_result = TerminalRuntime::spawn(config, emulator, grid.clone(), false, None);
        let (runtime, mut updates) = match spawn_result {
            Ok(value) => value,
            Err(err) => {
                if err
                    .chain()
                    .any(|cause| cause.to_string().to_lowercase().contains("permission"))
                {
                    eprintln!("skipping runtime_captures_command_output: {err}");
                    return;
                }
                panic!("spawn runtime: {err}");
            }
        };

        let mut collected = String::new();
        let mut timeout = Duration::from_secs(1);
        while collected.len() < 5 && timeout > Duration::from_millis(0) {
            if let Some(update) = updates.recv().await {
                match update {
                    CacheUpdate::Cell(cell) => {
                        let (ch, _) = crate::cache::terminal::unpack_cell(cell.cell);
                        collected.push(ch);
                    }
                    CacheUpdate::Row(row) => {
                        let line: String = row
                            .cells
                            .iter()
                            .map(|cell| unpack_cell(PackedCell::from(*cell)).0)
                            .collect();
                        collected.push_str(line.trim_end());
                    }
                    _ => {}
                }
            } else {
                break;
            }
            sleep(Duration::from_millis(10)).await;
            timeout = timeout.saturating_sub(Duration::from_millis(10));
        }

        assert!(collected.contains('h'));

        runtime.wait().await.expect("wait runtime");
    }

    #[test_timeout::tokio_timeout_test]
    async fn terminal_grid_retains_long_burst_output() {
        let rows = 24;
        let cols = 80;
        let grid = Arc::new(TerminalGrid::new(rows, cols));
        let emulator: Box<dyn TerminalEmulator + Send> =
            Box::new(SimpleTerminalEmulator::new(&grid));

        let command = Command::new("/bin/bash")
            .arg("-lc")
            .arg("for i in {1..150}; do echo \"Line $i: Test\"; done");
        let config = SpawnConfig::new(command, cols as u16, rows as u16);

        let spawn_result = TerminalRuntime::spawn(config, emulator, grid.clone(), false, None);
        let (runtime, mut updates) = match spawn_result {
            Ok(value) => value,
            Err(err) => {
                eprintln!("skipping terminal_grid_retains_long_burst_output: {err}");
                return;
            }
        };

        // Drain update stream until runtime completes.
        while let Some(update) = updates.recv().await {
            apply_update(&grid, &update);
        }
        runtime.wait().await.expect("wait runtime");

        let first_row = grid.first_row_id().unwrap_or(0);
        let last_row = grid.last_row_id().unwrap_or(first_row);
        let mut buffer = vec![0u64; grid.cols()];
        let mut missing: Vec<u64> = Vec::new();

        for absolute in first_row..=last_row {
            let Some(index) = grid.index_of_row(absolute) else {
                continue;
            };
            if grid.snapshot_row_into(index, &mut buffer).is_err() {
                continue;
            }
            let text: String = buffer
                .iter()
                .map(|cell| unpack_cell(PackedCell::from(*cell)).0)
                .collect();
            let trimmed = text.trim_end();
            if trimmed.starts_with("Line ") {
                continue;
            }
            if trimmed.is_empty() {
                missing.push(absolute);
            }
        }

        assert!(missing.is_empty(), "missing burst rows: {missing:?}");
    }
}

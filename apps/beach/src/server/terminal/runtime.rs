use crate::cache::terminal::TerminalGrid;
use crate::protocol::{HostFrame, ViewportCommand};
use crate::server::terminal::{
    Command as PtyCommand, PtyProcess, PtyWriter, SpawnConfig, TerminalEmulator,
};
use crate::sync::terminal::server_pipeline::{self, ForwarderCommand};
use crate::terminal::error::CliError;
use crate::transport::terminal::negotiation::SharedTransport;
use crate::transport::{Transport, TransportKind};
use anyhow::Result as AnyResult;
use crossterm::terminal::size as terminal_size;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, trace, warn};

pub(crate) const MAX_PTY_COLS: u16 = 200;
pub(crate) const MAX_PTY_ROWS: u16 = 200;

pub(crate) fn build_spawn_config(
    command: &[String],
) -> Result<(SpawnConfig, Arc<TerminalGrid>), CliError> {
    let mut iter = command.iter();
    let program = iter.next().cloned().ok_or(CliError::MissingCommand)?;
    let args: Vec<String> = iter.cloned().collect();

    let pty_command = PtyCommand::new(program)
        .args(args)
        .env("TERM", "xterm-256color");

    let (cols, rows) = detect_terminal_size();
    let grid = Arc::new(TerminalGrid::new(rows as usize, cols as usize));
    let config = SpawnConfig::new(pty_command, cols, rows);
    Ok((config, grid))
}

pub(crate) fn detect_terminal_size() -> (u16, u16) {
    if let Ok((cols, rows)) = terminal_size() {
        if cols > 0 && rows > 0 {
            return (cols.max(20), rows.max(10));
        }
    }

    let cols = std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(80);
    let rows = std::env::var("LINES")
        .or_else(|_| std::env::var("ROWS"))
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(24);
    (cols.max(20), rows.max(10))
}

pub(crate) fn handle_viewport_command(
    command: ViewportCommand,
    writer: &PtyWriter,
    transport_id: u64,
    transport_kind: &TransportKind,
    grid: &Arc<TerminalGrid>,
    forwarder_tx: &Option<UnboundedSender<ForwarderCommand>>,
) -> AnyResult<()> {
    match command {
        ViewportCommand::Clear => {
            writer.write(&[0x0c])?;
            grid.clear_viewport();
            debug!(
                target = "sync::incoming",
                transport_id,
                transport = ?transport_kind,
                "viewport clear command applied"
            );
            if let Some(tx) = forwarder_tx {
                if tx.send(ForwarderCommand::ViewportRefresh).is_err() {
                    trace!(
                        target = "sync::incoming",
                        transport_id, "viewport refresh send failed (receiver closed)"
                    );
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_local_resize_monitor(
    running: Arc<AtomicBool>,
    process: Arc<PtyProcess>,
    emulator: Arc<Mutex<Box<dyn TerminalEmulator + Send>>>,
    grid: Arc<TerminalGrid>,
    transports: Arc<Mutex<Vec<Arc<SharedTransport>>>>,
    local_server_transport: Arc<Mutex<Option<Arc<dyn Transport>>>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut last_size: Option<(u16, u16)> = None;
        while running.load(Ordering::Relaxed) {
            let (cols_raw, rows_raw) = detect_terminal_size();
            let cols = cols_raw.min(MAX_PTY_COLS);
            let rows = rows_raw.min(MAX_PTY_ROWS);
            let current = (cols, rows);

            if Some(current) != last_size {
                let mut applied = true;
                if let Err(err) = process.resize(cols, rows) {
                    warn!(
                        target = "host::local_resize",
                        cols,
                        rows,
                        error = %err,
                        "failed to apply PTY resize"
                    );
                    applied = false;
                }

                if applied {
                    if let Ok(mut emulator) = emulator.lock() {
                        emulator.resize(rows as usize, cols as usize);
                    }
                    grid.set_viewport_size(rows as usize, cols as usize);

                    broadcast_viewport(cols, rows, &grid, &transports, &local_server_transport);

                    trace!(
                        target = "host::local_resize",
                        cols, rows, "applied local terminal resize"
                    );

                    last_size = Some(current);
                }
            }

            thread::sleep(Duration::from_millis(200));
        }

        trace!(target = "host::local_resize", "resize monitor exiting");
    })
}

fn broadcast_viewport(
    cols: u16,
    rows: u16,
    grid: &Arc<TerminalGrid>,
    transports: &Arc<Mutex<Vec<Arc<SharedTransport>>>>,
    local_server_transport: &Arc<Mutex<Option<Arc<dyn Transport>>>>,
) {
    let history_rows = grid.rows() as u32;
    let base_row = grid.row_offset();

    let transports_snapshot: Vec<Arc<SharedTransport>> = {
        let guard = transports.lock().unwrap();
        guard.iter().cloned().collect()
    };

    for shared in transports_snapshot {
        let transport_id = shared.id().0;
        let transport_kind = shared.kind();
        let transport: Arc<dyn Transport> = shared;
        if let Err(err) = server_pipeline::send_host_frame(
            &transport,
            HostFrame::Grid {
                cols: cols as u32,
                history_rows,
                base_row,
                viewport_rows: Some(rows as u32),
            },
        ) {
            warn!(
                target = "host::local_resize",
                transport_id,
                transport = ?transport_kind,
                error = %err,
                "failed to broadcast viewport update"
            );
        }
    }

    if let Some(server) = local_server_transport.lock().unwrap().clone() {
        let transport_id = server.id().0;
        let transport_kind = server.kind();
        if let Err(err) = server_pipeline::send_host_frame(
            &server,
            HostFrame::Grid {
                cols: cols as u32,
                history_rows,
                base_row,
                viewport_rows: Some(rows as u32),
            },
        ) {
            warn!(
                target = "host::local_resize",
                transport_id,
                transport = ?transport_kind,
                error = %err,
                "failed to broadcast viewport update"
            );
        }
    }
}

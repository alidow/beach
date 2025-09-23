use crate::cache::Seq;
use crate::cache::terminal::{PackedCell, StyleId, unpack_cell};
use crate::client::grid_renderer::{GridRenderer, SelectionPosition};
use crate::protocol::{
    self, ClientFrame as WireClientFrame, HostFrame as WireHostFrame, Update as WireUpdate,
};
use crate::telemetry::{self, PerfGuard};
use crate::transport::{Payload, Transport, TransportError};
use copypasta::{ClipboardContext, ClipboardProvider};
use crossterm::{
    cursor::MoveTo,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{
        Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode,
    },
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::cmp;
use std::collections::HashMap;
use std::io::{self, IsTerminal, Write};
use std::sync::{
    Arc,
    mpsc::{Receiver, TryRecvError},
};
use std::time::{Duration, Instant};
use tracing::{Level, debug, trace};

#[derive(thiserror::Error, Debug)]
pub enum ClientError {
    #[error("transport error: {0}")]
    Transport(TransportError),
    #[error("protocol error: {0}")]
    Protocol(#[from] protocol::WireError),
    #[error("shutdown requested")]
    Shutdown,
}

pub struct TerminalClient {
    transport: Arc<dyn Transport>,
    renderer: GridRenderer,
    render_enabled: bool,
    tui: Option<Terminal<CrosstermBackend<io::Stdout>>>,
    last_seq: Seq,
    input_rx: Option<Receiver<Vec<u8>>>,
    input_seq: Seq,
    force_render: bool,
    cursor_row: usize,
    cursor_col: usize,
    pending_predictions: HashMap<Seq, Vec<(usize, usize)>>,
    copy_mode: Option<CopyModeState>,
    last_render_at: Option<Instant>,
    render_interval: Duration,
    pending_render: bool,
    predictive_input: bool,
}

impl TerminalClient {
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        let render_enabled = io::stdout().is_terminal();
        let mut renderer = GridRenderer::new(0, 0);
        renderer.on_resize(80, 24);
        Self {
            transport,
            renderer,
            render_enabled,
            tui: None,
            last_seq: 0,
            input_rx: None,
            input_seq: 0,
            force_render: true,
            cursor_row: 0,
            cursor_col: 0,
            pending_predictions: HashMap::new(),
            copy_mode: None,
            last_render_at: None,
            render_interval: Duration::from_millis(16),
            pending_render: false,
            predictive_input: false,
        }
    }

    pub fn with_render(mut self, enabled: bool) -> Self {
        self.render_enabled = enabled;
        self
    }

    pub fn with_input(mut self, rx: Receiver<Vec<u8>>) -> Self {
        self.input_rx = Some(rx);
        self
    }

    pub fn with_predictive_input(mut self, enabled: bool) -> Self {
        self.predictive_input = enabled;
        self
    }

    pub fn run(mut self) -> Result<(), ClientError> {
        self.setup_tui()?;
        debug!(target = "client::loop", "client loop started");
        loop {
            self.pump_input()?;
            let message = match self.transport.recv(Duration::from_millis(25)) {
                Ok(message) => Some(message),
                Err(TransportError::Timeout) => None,
                Err(TransportError::ChannelClosed) => break,
                Err(err) => return Err(ClientError::Transport(err)),
            };

            if let Some(message) = message {
                match message.payload {
                    Payload::Binary(bytes) => {
                        telemetry::record_bytes("client_frame_bytes", bytes.len());
                        let decode_start = Instant::now();
                        let frame = protocol::decode_host_frame_binary(&bytes)?;
                        let decode_elapsed = decode_start.elapsed();
                        match &frame {
                            WireHostFrame::Snapshot { .. } => {
                                telemetry::record_duration("client_decode_snapshot", decode_elapsed)
                            }
                            WireHostFrame::Delta { .. } => {
                                telemetry::record_duration("client_decode_delta", decode_elapsed)
                            }
                            _ => telemetry::record_duration("client_decode_frame", decode_elapsed),
                        }
                        match self.handle_host_frame(frame) {
                            Ok(()) => {}
                            Err(ClientError::Shutdown) => break,
                            Err(err) => return Err(err),
                        }
                    }
                    Payload::Text(text) => {
                        telemetry::record_bytes("client_frame_bytes", text.len());
                        let trimmed = text.trim();
                        if trimmed == "__ready__" || trimmed == "__offer_ready__" {
                            trace!(
                                target = "client::frame",
                                payload = trimmed,
                                "ignoring handshake sentinel"
                            );
                        } else {
                            debug!(target = "client::frame", payload = %trimmed, "unexpected text payload");
                        }
                    }
                }
            }

            self.maybe_render()?;
        }
        self.teardown_tui()?;
        debug!(target = "client::loop", "client loop stopped");
        Ok(())
    }

    fn handle_host_frame(&mut self, frame: WireHostFrame) -> Result<(), ClientError> {
        if tracing::enabled!(Level::DEBUG) {
            let frame_type = match &frame {
                WireHostFrame::Heartbeat { .. } => "heartbeat",
                WireHostFrame::Hello { .. } => "hello",
                WireHostFrame::Grid { .. } => "grid",
                WireHostFrame::Snapshot { .. } => "snapshot",
                WireHostFrame::SnapshotComplete { .. } => "snapshot_complete",
                WireHostFrame::Delta { .. } => "delta",
                WireHostFrame::InputAck { .. } => "input_ack",
                WireHostFrame::Shutdown => "shutdown",
            };
            debug!(
                target = "client::frame",
                frame = frame_type,
                "processing binary frame"
            );
        }

        let _guard = PerfGuard::new("client_handle_frame_binary");
        match frame {
            WireHostFrame::Heartbeat { .. } => {}
            WireHostFrame::Hello { .. } => {}
            WireHostFrame::Grid { rows, cols } => {
                let rows = rows as usize;
                let cols = cols as usize;
                self.renderer.ensure_size(rows, cols);
                self.renderer.mark_dirty();
                self.force_render = true;
                self.cursor_row = rows.saturating_sub(1);
                self.cursor_col = 0;
                self.renderer.clear_all_predictions();
                self.pending_predictions.clear();
            }
            WireHostFrame::Snapshot {
                updates, watermark, ..
            }
            | WireHostFrame::Delta {
                updates, watermark, ..
            } => {
                for update in &updates {
                    self.apply_wire_update(update);
                }
                self.last_seq = cmp::max(self.last_seq, watermark);
                self.force_render = true;
            }
            WireHostFrame::InputAck { seq } => {
                self.handle_input_ack(seq);
            }
            WireHostFrame::SnapshotComplete { .. } => {}
            WireHostFrame::Shutdown => return Err(ClientError::Shutdown),
        }
        Ok(())
    }

    fn apply_wire_update(&mut self, update: &WireUpdate) {
        use CursorHint::*;

        let cursor_hint = match update {
            WireUpdate::Cell { row, col, .. } => {
                Some(Exact(*row as usize, (*col as usize).saturating_add(1)))
            }
            WireUpdate::Row { row, .. } => Some(RowWidth(*row as usize)),
            WireUpdate::Rect { rows, cols, .. } => {
                let target_row = rows[1] as usize;
                let target_col = cols[1] as usize;
                Some(Exact(target_row.saturating_sub(1), target_col))
            }
            WireUpdate::RowSegment {
                row,
                start_col,
                cells,
                ..
            } => cells
                .iter()
                .enumerate()
                .map(|(idx, _)| Exact(*row as usize, (*start_col as usize).saturating_add(idx + 1)))
                .last(),
            WireUpdate::Trim { .. } => None,
            WireUpdate::Style { .. } => None,
        };

        match update {
            WireUpdate::Cell {
                row,
                col,
                seq,
                cell,
            } => {
                let (ch, style) = decode_wire_cell(*cell);
                self.renderer
                    .apply_cell(*row as usize, *col as usize, *seq, ch, style);
            }
            WireUpdate::Row { row, seq, cells } => {
                let row_index = *row as usize;
                for (col, cell) in cells.iter().enumerate() {
                    let (ch, style) = decode_wire_cell(*cell);
                    self.renderer.apply_cell(row_index, col, *seq, ch, style);
                }
            }
            WireUpdate::Rect {
                rows,
                cols,
                seq,
                cell,
            } => {
                let row_range = rows[0] as usize..rows[1] as usize;
                let col_range = cols[0] as usize..cols[1] as usize;
                let (ch, style) = decode_wire_cell(*cell);
                self.renderer
                    .apply_rect(row_range, col_range, *seq, ch, style);
            }
            WireUpdate::RowSegment {
                row,
                start_col,
                seq,
                cells,
            } => {
                if !cells.is_empty() {
                    let mut segment = Vec::with_capacity(cells.len());
                    for (idx, cell) in cells.iter().enumerate() {
                        let (ch, style) = decode_wire_cell(*cell);
                        let col = *start_col as usize + idx;
                        segment.push((col, *seq, ch, style));
                    }
                    self.renderer.apply_segment(*row as usize, &segment);
                }
            }
            WireUpdate::Trim { start, count, .. } => {
                let start = *start as usize;
                let count = *count as usize;
                self.renderer.apply_trim(start, count);
                self.pending_predictions.values_mut().for_each(|positions| {
                    positions.retain(|(row, _)| *row >= start + count);
                });
                self.pending_predictions
                    .retain(|_, positions| !positions.is_empty());
                if self.cursor_row >= start && self.cursor_row < start + count {
                    self.cursor_row = start + count;
                    self.cursor_col = 0;
                }
                self.force_render = true;
            }
            WireUpdate::Style {
                id,
                seq,
                fg,
                bg,
                attrs,
            } => {
                self.renderer.set_style(*id, *fg, *bg, *attrs);
                self.last_seq = cmp::max(self.last_seq, *seq);
            }
        }

        if let Some(hint) = cursor_hint {
            match hint {
                Exact(row, col) => {
                    self.cursor_row = row;
                    self.cursor_col = col;
                }
                RowWidth(row) => {
                    self.cursor_row = row;
                    self.cursor_col = self.renderer.row_display_width(row);
                }
            }
        }
    }

    fn maybe_render(&mut self) -> Result<(), ClientError> {
        if !self.render_enabled {
            return Ok(());
        }

        if self.pending_render {
            let ready = self
                .last_render_at
                .map(|last| last.elapsed() >= self.render_interval)
                .unwrap_or(true);
            if ready {
                self.pending_render = false;
                self.force_render = false;
                self.render()?;
                self.last_render_at = Some(Instant::now());
            }
            return Ok(());
        }

        let dirty = self.renderer.take_dirty();
        if self.force_render || dirty {
            let now = Instant::now();
            if !self.force_render {
                if let Some(last) = self.last_render_at {
                    if now.duration_since(last) < self.render_interval {
                        self.pending_render = true;
                        if dirty {
                            self.renderer.mark_dirty();
                        }
                        return Ok(());
                    }
                }
            }
            self.force_render = false;
            self.render()?;
            self.last_render_at = Some(now);
        }
        Ok(())
    }

    fn render(&mut self) -> Result<(), ClientError> {
        if let Some(tui) = &mut self.tui {
            let _guard = PerfGuard::new("client_render_tui");
            let renderer = &mut self.renderer;
            tui.draw(|frame| renderer.render_frame(frame))
                .map_err(|err| ClientError::Transport(TransportError::Setup(err.to_string())))?;
        } else {
            let _guard = PerfGuard::new("client_render_simple");
            let mut stdout = io::stdout();
            execute!(stdout, MoveTo(0, 0), Clear(ClearType::All))
                .map_err(|err| ClientError::Transport(TransportError::Setup(err.to_string())))?;
            for line in self.renderer.visible_lines() {
                writeln!(stdout, "{}", line).map_err(|err| {
                    ClientError::Transport(TransportError::Setup(err.to_string()))
                })?;
            }
            stdout
                .flush()
                .map_err(|err| ClientError::Transport(TransportError::Setup(err.to_string())))?;
        }
        Ok(())
    }

    fn pump_input(&mut self) -> Result<(), ClientError> {
        let mut pending: Vec<Vec<u8>> = Vec::new();
        let mut disconnected = false;

        if let Some(rx) = &self.input_rx {
            loop {
                match rx.try_recv() {
                    Ok(bytes) => pending.push(bytes),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        } else if self.render_enabled {
            while event::poll(Duration::from_millis(0)).unwrap_or(false) {
                match event::read() {
                    Ok(Event::Key(key)) => {
                        if self.handle_control_shortcuts(&key)? {
                            continue;
                        }
                        if self.process_copy_mode_key(&key) {
                            continue;
                        }
                        if self.handle_local_key(&key) {
                            continue;
                        }
                        if let Some(bytes) = encode_key_event(key) {
                            pending.push(bytes);
                        }
                    }
                    Ok(Event::Paste(data)) => {
                        pending.push(data.into_bytes());
                    }
                    Ok(Event::Resize(cols, rows)) => {
                        self.renderer.on_resize(cols, rows);
                        self.force_render = true;
                        self.send_resize(cols, rows)?;
                    }
                    Ok(Event::Mouse(_)) => {}
                    Err(err) => {
                        eprintln!("⚠️  input read error: {err}");
                        break;
                    }
                    _ => {}
                }
            }
        }

        if disconnected {
            self.input_rx = None;
        }

        for bytes in pending {
            self.send_input(&bytes)?;
        }
        Ok(())
    }

    fn process_copy_mode_key(&mut self, key: &KeyEvent) -> bool {
        if self.copy_mode.is_some() {
            if key.code == KeyCode::Esc
                || (key.modifiers.contains(KeyModifiers::ALT)
                    && matches!(key.code, KeyCode::Char(']') | KeyCode::Char('}')))
            {
                self.exit_copy_mode();
                return true;
            }
            if key.modifiers.contains(KeyModifiers::ALT) {
                if let KeyCode::Char(c) = key.code {
                    if c.to_ascii_lowercase() == 'y' {
                        self.copy_selection_to_clipboard();
                        return true;
                    }
                }
            }
            match key.code {
                KeyCode::Up => {
                    self.move_copy_cursor(-1, 0);
                    return true;
                }
                KeyCode::Down => {
                    self.move_copy_cursor(1, 0);
                    return true;
                }
                KeyCode::Left => {
                    self.move_copy_cursor(0, -1);
                    return true;
                }
                KeyCode::Right => {
                    self.move_copy_cursor(0, 1);
                    return true;
                }
                KeyCode::PageUp => {
                    self.move_copy_cursor_page(-1);
                    return true;
                }
                KeyCode::PageDown => {
                    self.move_copy_cursor_page(1);
                    return true;
                }
                KeyCode::Home => {
                    self.move_copy_cursor_line_start();
                    return true;
                }
                KeyCode::End => {
                    self.move_copy_cursor_line_end();
                    return true;
                }
                _ => {}
            }

            // If copy mode active but key not handled, fall through to default handling
        } else if key.modifiers.contains(KeyModifiers::ALT) {
            if let KeyCode::Char(c) = key.code {
                if c.to_ascii_lowercase() == '[' {
                    self.enter_copy_mode();
                    return true;
                }
            }
        }

        false
    }

    fn enter_copy_mode(&mut self) {
        if self.copy_mode.is_some() {
            return;
        }
        let total_rows = self.renderer.total_rows();
        let total_cols = self.renderer.total_cols();
        if total_rows == 0 || total_cols == 0 {
            return;
        }
        let viewport_top = self.renderer.viewport_top();
        let viewport_height = self.renderer.viewport_height();
        let max_row = total_rows.saturating_sub(1);
        let start_row = (viewport_top + viewport_height.saturating_sub(1)).min(max_row);
        let start_pos = self.renderer.clamp_position(start_row as isize, 0);
        self.copy_mode = Some(CopyModeState::new(start_pos));
        self.renderer.set_follow_tail(false);
        self.renderer.set_selection(start_pos, start_pos);
        self.force_render = true;
    }

    fn exit_copy_mode(&mut self) {
        if self.copy_mode.take().is_some() {
            self.renderer.clear_selection();
            self.renderer.set_follow_tail(true);
            self.renderer.mark_dirty();
            self.force_render = true;
        }
    }

    fn move_copy_cursor(&mut self, delta_row: isize, delta_col: isize) {
        if let Some(state) = &mut self.copy_mode {
            let new_row = state.cursor.row as isize + delta_row;
            let new_col = state.cursor.col as isize + delta_col;
            let new_pos = self.renderer.clamp_position(new_row, new_col);
            state.cursor = new_pos;
            self.renderer.set_selection(state.anchor, new_pos);
            self.renderer.set_follow_tail(false);
            self.force_render = true;
        }
    }

    fn move_copy_cursor_page(&mut self, pages: isize) {
        if pages == 0 {
            return;
        }
        let step = self.renderer.viewport_height() as isize;
        if step == 0 {
            return;
        }
        self.move_copy_cursor(pages * step, 0);
    }

    fn move_copy_cursor_line_start(&mut self) {
        if let Some(state) = &mut self.copy_mode {
            let new_pos = self.renderer.clamp_position(state.cursor.row as isize, 0);
            state.cursor = new_pos;
            self.renderer.set_selection(state.anchor, new_pos);
            self.force_render = true;
        }
    }

    fn move_copy_cursor_line_end(&mut self) {
        if let Some(state) = &mut self.copy_mode {
            let row = state.cursor.row;
            let row_width = self.renderer.row_display_width(row);
            let target_col = if row_width == 0 { 0 } else { row_width - 1 };
            let new_pos = self
                .renderer
                .clamp_position(row as isize, target_col as isize);
            state.cursor = new_pos;
            self.renderer.set_selection(state.anchor, new_pos);
            self.force_render = true;
        }
    }

    fn copy_selection_to_clipboard(&mut self) {
        if let Some(text) = self.renderer.selection_text() {
            match ClipboardContext::new() {
                Ok(mut ctx) => {
                    if let Err(err) = ctx.set_contents(text.clone()) {
                        eprintln!("⚠️  failed to copy selection: {err}");
                    }
                }
                Err(err) => eprintln!("⚠️  clipboard unavailable: {err}"),
            }
        }
        self.exit_copy_mode();
    }

    fn handle_control_shortcuts(&mut self, key: &KeyEvent) -> Result<bool, ClientError> {
        if !key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(false);
        }
        match key.code {
            KeyCode::Char(c) if c.eq_ignore_ascii_case(&'q') => {
                return Err(ClientError::Shutdown);
            }
            KeyCode::Char(c) if c.eq_ignore_ascii_case(&'c') && self.copy_mode.is_some() => {
                self.exit_copy_mode();
                return Ok(true);
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_local_key(&mut self, key: &KeyEvent) -> bool {
        if !key.modifiers.intersects(KeyModifiers::ALT) {
            return false;
        }
        let lower = match key.code {
            KeyCode::Char(c) => c.to_ascii_lowercase(),
            _ => '\0',
        };
        match key.code {
            KeyCode::Up => {
                self.renderer.set_follow_tail(false);
                self.renderer.scroll_lines(-1);
                self.force_render = true;
                true
            }
            KeyCode::Down => {
                self.renderer.scroll_lines(1);
                self.force_render = true;
                true
            }
            KeyCode::PageUp => {
                self.renderer.set_follow_tail(false);
                self.renderer.scroll_pages(-1);
                self.force_render = true;
                true
            }
            KeyCode::PageDown => {
                self.renderer.scroll_pages(1);
                self.force_render = true;
                true
            }
            KeyCode::End => {
                self.renderer.scroll_to_tail();
                self.force_render = true;
                true
            }
            KeyCode::Home => {
                self.renderer.scroll_to_top();
                self.force_render = true;
                true
            }
            KeyCode::Char(_) if lower == 'f' => {
                self.renderer.toggle_follow_tail();
                self.force_render = true;
                true
            }
            KeyCode::Char(_) if lower == 'c' => {
                self.renderer.clear_selection();
                self.force_render = true;
                true
            }
            _ => false,
        }
    }

    fn send_input(&mut self, bytes: &[u8]) -> Result<(), ClientError> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.input_seq = self.input_seq.saturating_add(1);
        telemetry::record_bytes("client_input_bytes", bytes.len());
        let frame = WireClientFrame::Input {
            seq: self.input_seq,
            data: bytes.to_vec(),
        };
        let encoded = protocol::encode_client_frame_binary(&frame);
        telemetry::record_bytes("client_input_frames", encoded.len());
        self.transport
            .send_bytes(&encoded)
            .map_err(ClientError::Transport)?;
        if tracing::enabled!(Level::TRACE) {
            trace!(
                target = "client::outgoing",
                seq = self.input_seq,
                bytes = bytes.len(),
                dump = %crate::telemetry::logging::hexdump(bytes),
                "input sent"
            );
        }
        self.register_prediction(self.input_seq, bytes);
        Ok(())
    }

    fn send_resize(&mut self, cols: u16, rows: u16) -> Result<(), ClientError> {
        let frame = WireClientFrame::Resize { cols, rows };
        let encoded = protocol::encode_client_frame_binary(&frame);
        telemetry::record_bytes("client_input_frames", encoded.len());
        self.transport
            .send_bytes(&encoded)
            .map_err(ClientError::Transport)?;
        debug!(target = "client::outgoing", cols, rows, "resize sent");
        Ok(())
    }

    fn handle_input_ack(&mut self, seq: Seq) {
        self.renderer.clear_prediction_seq(seq);
        self.pending_predictions.remove(&seq);
        self.force_render = true;
    }

    fn register_prediction(&mut self, seq: Seq, bytes: &[u8]) {
        if !self.render_enabled || !self.predictive_input {
            return;
        }
        if bytes.len() > 32 {
            return;
        }
        if self.pending_predictions.len() > 256 {
            self.pending_predictions.clear();
            self.renderer.clear_all_predictions();
        }
        let mut positions = Vec::new();
        for &byte in bytes {
            match byte {
                b'\r' => {
                    self.cursor_col = 0;
                }
                b'\n' => {
                    self.cursor_row = self.cursor_row.saturating_add(1);
                    self.cursor_col = 0;
                }
                0x00..=0x1f | 0x7f => {}
                value => {
                    let ch = value as char;
                    let row = self.cursor_row;
                    let col = self.cursor_col;
                    self.renderer.add_prediction(row, col, seq, ch);
                    positions.push((row, col));
                    self.advance_cursor_for_char(ch);
                }
            }
        }
        if !positions.is_empty() {
            self.pending_predictions.insert(seq, positions);
            self.force_render = true;
        }
    }

    fn advance_cursor_for_char(&mut self, ch: char) {
        if ch == '\n' {
            self.cursor_row = self.cursor_row.saturating_add(1);
            self.cursor_col = 0;
        } else {
            self.cursor_col = self.cursor_col.saturating_add(1);
        }
    }

    fn setup_tui(&mut self) -> Result<(), ClientError> {
        if !self.render_enabled {
            self.force_render = true;
            self.renderer.mark_dirty();
            return Ok(());
        }
        enable_raw_mode()
            .map_err(|err| ClientError::Transport(TransportError::Setup(err.to_string())))?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)
            .map_err(|err| ClientError::Transport(TransportError::Setup(err.to_string())))?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)
            .map_err(|err| ClientError::Transport(TransportError::Setup(err.to_string())))?;
        terminal.hide_cursor().ok();
        if let Ok(area) = terminal.size() {
            self.renderer.on_resize(area.width, area.height);
        }
        self.renderer.mark_dirty();
        self.force_render = true;
        self.tui = Some(terminal);
        Ok(())
    }

    fn teardown_tui(&mut self) -> Result<(), ClientError> {
        if !self.render_enabled {
            return Ok(());
        }
        if let Some(mut terminal) = self.tui.take() {
            terminal.show_cursor().ok();
            terminal
                .clear()
                .map_err(|err| ClientError::Transport(TransportError::Setup(err.to_string())))?;
        }
        disable_raw_mode()
            .map_err(|err| ClientError::Transport(TransportError::Setup(err.to_string())))?;
        let mut stdout = io::stdout();
        execute!(stdout, LeaveAlternateScreen)
            .map_err(|err| ClientError::Transport(TransportError::Setup(err.to_string())))?;
        Ok(())
    }
}

fn decode_wire_cell(cell: u64) -> (char, Option<u32>) {
    let packed = PackedCell::from(cell);
    let (ch, style_id) = unpack_cell(packed);
    if style_id == StyleId::DEFAULT {
        (ch, None)
    } else {
        (ch, Some(style_id.0))
    }
}

#[derive(Clone, Copy, Debug)]
struct CopyModeState {
    anchor: SelectionPosition,
    cursor: SelectionPosition,
}

impl CopyModeState {
    fn new(anchor: SelectionPosition) -> Self {
        Self {
            anchor,
            cursor: anchor,
        }
    }
}

enum CursorHint {
    Exact(usize, usize),
    RowWidth(usize),
}

fn encode_key_event(key: KeyEvent) -> Option<Vec<u8>> {
    match key.code {
        KeyCode::Char(c) => {
            let mut bytes = Vec::new();
            if key.modifiers.contains(KeyModifiers::ALT) {
                bytes.push(0x1b);
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                let lower = c.to_ascii_lowercase();
                if ('a'..='z').contains(&lower) {
                    bytes.push((lower as u8 - b'a') + 1);
                } else {
                    return None;
                }
            } else {
                bytes.push(c as u8);
            }
            Some(bytes)
        }
        KeyCode::Enter => Some(vec![b'\n']),
        KeyCode::Tab => Some(vec![b'\t']),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::Home => Some(b"\x1b[H".to_vec()),
        KeyCode::End => Some(b"\x1b[F".to_vec()),
        KeyCode::PageUp => Some(b"\x1b[5~".to_vec()),
        KeyCode::PageDown => Some(b"\x1b[6~".to_vec()),
        KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
        KeyCode::Insert => Some(b"\x1b[2~".to_vec()),
        _ => None,
    }
}

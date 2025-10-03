use super::{
    ClientFrame, CursorFrame, HostFrame, Lane, LaneBudgetFrame, PROTOCOL_VERSION, SyncConfigFrame,
    Update, ViewportCommand,
};

const VERSION_BITS: u8 = 3;
const VERSION_MASK: u8 = 0b1110_0000;
const TYPE_MASK: u8 = 0b0001_1111;

const HOST_KIND_HEARTBEAT: u8 = 0;
const HOST_KIND_HELLO: u8 = 1;
const HOST_KIND_GRID: u8 = 2;
const HOST_KIND_SNAPSHOT: u8 = 3;
const HOST_KIND_SNAPSHOT_COMPLETE: u8 = 4;
const HOST_KIND_DELTA: u8 = 5;
const HOST_KIND_INPUT_ACK: u8 = 6;
const HOST_KIND_SHUTDOWN: u8 = 7;
const HOST_KIND_HISTORY_BACKFILL: u8 = 8;
const HOST_KIND_CURSOR: u8 = 9;

const UPDATE_KIND_CELL: u8 = 0;
const UPDATE_KIND_RECT: u8 = 1;
const UPDATE_KIND_ROW: u8 = 2;
const UPDATE_KIND_SEGMENT: u8 = 3;
const UPDATE_KIND_TRIM: u8 = 4;
const UPDATE_KIND_STYLE: u8 = 5;

const CLIENT_KIND_INPUT: u8 = 0;
const CLIENT_KIND_RESIZE: u8 = 1;
const CLIENT_KIND_REQUEST_BACKFILL: u8 = 2;
const CLIENT_KIND_VIEWPORT_COMMAND: u8 = 3;
const CLIENT_KIND_UNKNOWN: u8 = TYPE_MASK;

const ENV_BINARY_PROTOCOL: &str = "BEACH_PROTO_BINARY";

/// Returns `true` when the binary protocol should be used instead of the legacy JSON path.
///
/// By default the binary protocol is disabled until the server/client pipelines are migrated.
pub fn binary_protocol_enabled() -> bool {
    std::env::var(ENV_BINARY_PROTOCOL)
        .map(|value| parse_flag(&value))
        .unwrap_or(false)
}

fn parse_flag(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WireError {
    #[error("invalid protocol version: {0}")]
    InvalidVersion(u8),
    #[error("unknown frame type: {0}")]
    UnknownFrameType(u8),
    #[error("unknown update tag: {0}")]
    UnknownUpdateTag(u8),
    #[error("unexpected end of input")]
    UnexpectedEof,
    #[error("varint overflow")]
    VarIntOverflow,
    #[error("invalid data: {0}")]
    InvalidData(&'static str),
}

pub fn encode_host_frame_binary(frame: &HostFrame) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);
    match frame {
        HostFrame::Heartbeat { seq, timestamp_ms } => {
            write_header(&mut buf, HOST_KIND_HEARTBEAT);
            write_var_u64(&mut buf, *seq);
            write_var_u64(&mut buf, *timestamp_ms);
        }
        HostFrame::Hello {
            subscription,
            max_seq,
            config,
            features,
        } => {
            write_header(&mut buf, HOST_KIND_HELLO);
            write_var_u64(&mut buf, *subscription);
            write_var_u64(&mut buf, *max_seq);
            encode_sync_config(&mut buf, config);
            write_var_u32(&mut buf, *features);
        }
        HostFrame::Grid {
            cols,
            history_rows,
            base_row,
            viewport_rows,
        } => {
            write_header(&mut buf, HOST_KIND_GRID);
            if let Some(rows) = viewport_rows {
                write_var_u32(&mut buf, *rows);
            }
            write_var_u32(&mut buf, *cols);
            write_var_u32(&mut buf, *history_rows);
            write_var_u64(&mut buf, *base_row);
        }
        HostFrame::Snapshot {
            subscription,
            lane,
            watermark,
            has_more,
            updates,
            cursor,
        } => {
            write_header(&mut buf, HOST_KIND_SNAPSHOT);
            write_var_u64(&mut buf, *subscription);
            buf.push(lane.as_u8());
            write_var_u64(&mut buf, *watermark);
            buf.push(*has_more as u8);
            encode_updates(&mut buf, updates);
            buf.push(cursor.is_some() as u8);
            if let Some(frame) = cursor {
                encode_cursor(&mut buf, frame);
            }
        }
        HostFrame::SnapshotComplete { subscription, lane } => {
            write_header(&mut buf, HOST_KIND_SNAPSHOT_COMPLETE);
            write_var_u64(&mut buf, *subscription);
            buf.push(lane.as_u8());
        }
        HostFrame::Delta {
            subscription,
            watermark,
            has_more,
            updates,
            cursor,
        } => {
            write_header(&mut buf, HOST_KIND_DELTA);
            write_var_u64(&mut buf, *subscription);
            write_var_u64(&mut buf, *watermark);
            buf.push(*has_more as u8);
            encode_updates(&mut buf, updates);
            buf.push(cursor.is_some() as u8);
            if let Some(frame) = cursor {
                encode_cursor(&mut buf, frame);
            }
        }
        HostFrame::HistoryBackfill {
            subscription,
            request_id,
            start_row,
            count,
            updates,
            more,
            cursor,
        } => {
            write_header(&mut buf, HOST_KIND_HISTORY_BACKFILL);
            write_var_u64(&mut buf, *subscription);
            write_var_u64(&mut buf, *request_id);
            write_var_u64(&mut buf, *start_row);
            write_var_u32(&mut buf, *count);
            buf.push(*more as u8);
            encode_updates(&mut buf, updates);
            buf.push(cursor.is_some() as u8);
            if let Some(frame) = cursor {
                encode_cursor(&mut buf, frame);
            }
        }
        HostFrame::InputAck { seq } => {
            write_header(&mut buf, HOST_KIND_INPUT_ACK);
            write_var_u64(&mut buf, *seq);
        }
        HostFrame::Cursor {
            subscription,
            cursor,
        } => {
            write_header(&mut buf, HOST_KIND_CURSOR);
            write_var_u64(&mut buf, *subscription);
            encode_cursor(&mut buf, cursor);
        }
        HostFrame::Shutdown => {
            write_header(&mut buf, HOST_KIND_SHUTDOWN);
        }
    }
    buf
}

pub fn decode_host_frame_binary(bytes: &[u8]) -> Result<HostFrame, WireError> {
    let mut cursor = Cursor::new(bytes);
    let (kind, _) = read_header(&mut cursor)?;
    match kind {
        HOST_KIND_HEARTBEAT => {
            let seq = cursor.read_var_u64()?;
            let timestamp_ms = cursor.read_var_u64()?;
            Ok(HostFrame::Heartbeat { seq, timestamp_ms })
        }
        HOST_KIND_HELLO => {
            let subscription = cursor.read_var_u64()?;
            let max_seq = cursor.read_var_u64()?;
            let config = decode_sync_config(&mut cursor)?;
            let features = cursor.read_var_u32()?;
            Ok(HostFrame::Hello {
                subscription,
                max_seq,
                config,
                features,
            })
        }
        HOST_KIND_GRID => {
            let checkpoint = cursor;
            let cols = cursor.read_var_u32()?;
            let history_rows = cursor.read_var_u32()?;
            let base_row = cursor.read_var_u64()?;
            if cursor.remaining() == 0 {
                Ok(HostFrame::Grid {
                    cols,
                    history_rows,
                    base_row,
                    viewport_rows: None,
                })
            } else {
                let mut legacy = checkpoint;
                let viewport_rows = legacy.read_var_u32()?;
                let cols = legacy.read_var_u32()?;
                let history_rows = legacy.read_var_u32()?;
                let base_row = legacy.read_var_u64()?;
                Ok(HostFrame::Grid {
                    cols,
                    history_rows,
                    base_row,
                    viewport_rows: Some(viewport_rows),
                })
            }
        }
        HOST_KIND_SNAPSHOT => {
            let subscription = cursor.read_var_u64()?;
            let lane = decode_lane(&mut cursor)?;
            let watermark = cursor.read_var_u64()?;
            let has_more = cursor.read_bool()?;
            let updates = decode_updates(&mut cursor)?;
            let cursor_frame = if cursor.remaining() > 0 {
                let has_cursor = cursor.read_bool()?;
                if has_cursor {
                    Some(decode_cursor(&mut cursor)?)
                } else {
                    None
                }
            } else {
                None
            };
            Ok(HostFrame::Snapshot {
                subscription,
                lane,
                watermark,
                has_more,
                updates,
                cursor: cursor_frame,
            })
        }
        HOST_KIND_SNAPSHOT_COMPLETE => {
            let subscription = cursor.read_var_u64()?;
            let lane = decode_lane(&mut cursor)?;
            Ok(HostFrame::SnapshotComplete { subscription, lane })
        }
        HOST_KIND_DELTA => {
            let subscription = cursor.read_var_u64()?;
            let watermark = cursor.read_var_u64()?;
            let has_more = cursor.read_bool()?;
            let updates = decode_updates(&mut cursor)?;
            let cursor_frame = if cursor.remaining() > 0 {
                let has_cursor = cursor.read_bool()?;
                if has_cursor {
                    Some(decode_cursor(&mut cursor)?)
                } else {
                    None
                }
            } else {
                None
            };
            Ok(HostFrame::Delta {
                subscription,
                watermark,
                has_more,
                updates,
                cursor: cursor_frame,
            })
        }
        HOST_KIND_HISTORY_BACKFILL => {
            let subscription = cursor.read_var_u64()?;
            let request_id = cursor.read_var_u64()?;
            let start_row = cursor.read_var_u64()?;
            let count = cursor.read_var_u32()?;
            let more = cursor.read_bool()?;
            let updates = decode_updates(&mut cursor)?;
            let cursor_frame = if cursor.remaining() > 0 {
                let has_cursor = cursor.read_bool()?;
                if has_cursor {
                    Some(decode_cursor(&mut cursor)?)
                } else {
                    None
                }
            } else {
                None
            };
            Ok(HostFrame::HistoryBackfill {
                subscription,
                request_id,
                start_row,
                count,
                updates,
                more,
                cursor: cursor_frame,
            })
        }
        HOST_KIND_INPUT_ACK => {
            let seq = cursor.read_var_u64()?;
            Ok(HostFrame::InputAck { seq })
        }
        HOST_KIND_CURSOR => {
            let subscription = cursor.read_var_u64()?;
            let cursor_frame = decode_cursor(&mut cursor)?;
            Ok(HostFrame::Cursor {
                subscription,
                cursor: cursor_frame,
            })
        }
        HOST_KIND_SHUTDOWN => Ok(HostFrame::Shutdown),
        other => Err(WireError::UnknownFrameType(other)),
    }
}

pub fn encode_client_frame_binary(frame: &ClientFrame) -> Vec<u8> {
    let mut buf = Vec::with_capacity(32);
    match frame {
        ClientFrame::Input { seq, data } => {
            write_header(&mut buf, CLIENT_KIND_INPUT);
            write_var_u64(&mut buf, *seq);
            write_var_u32(&mut buf, data.len() as u32);
            buf.extend_from_slice(data);
        }
        ClientFrame::Resize { cols, rows } => {
            write_header(&mut buf, CLIENT_KIND_RESIZE);
            write_var_u32(&mut buf, (*cols).into());
            write_var_u32(&mut buf, (*rows).into());
        }
        ClientFrame::RequestBackfill {
            subscription,
            request_id,
            start_row,
            count,
        } => {
            write_header(&mut buf, CLIENT_KIND_REQUEST_BACKFILL);
            write_var_u64(&mut buf, *subscription);
            write_var_u64(&mut buf, *request_id);
            write_var_u64(&mut buf, *start_row);
            write_var_u32(&mut buf, *count);
        }
        ClientFrame::ViewportCommand { command } => {
            write_header(&mut buf, CLIENT_KIND_VIEWPORT_COMMAND);
            buf.push(command.as_u8());
        }
        ClientFrame::Unknown => {
            write_header(&mut buf, CLIENT_KIND_UNKNOWN);
        }
    }
    buf
}

pub fn decode_client_frame_binary(bytes: &[u8]) -> Result<ClientFrame, WireError> {
    let mut cursor = Cursor::new(bytes);
    let (kind, _) = read_header(&mut cursor)?;
    match kind {
        CLIENT_KIND_INPUT => {
            let seq = cursor.read_var_u64()?;
            let len = cursor.read_var_u32()? as usize;
            let data = cursor.read_bytes(len)?.to_vec();
            Ok(ClientFrame::Input { seq, data })
        }
        CLIENT_KIND_RESIZE => {
            let cols = cursor.read_var_u32()? as u16;
            let rows = cursor.read_var_u32()? as u16;
            Ok(ClientFrame::Resize { cols, rows })
        }
        CLIENT_KIND_REQUEST_BACKFILL => {
            let subscription = cursor.read_var_u64()?;
            let request_id = cursor.read_var_u64()?;
            let start_row = cursor.read_var_u64()?;
            let count = cursor.read_var_u32()?;
            Ok(ClientFrame::RequestBackfill {
                subscription,
                request_id,
                start_row,
                count,
            })
        }
        CLIENT_KIND_VIEWPORT_COMMAND => {
            let code = cursor.read_u8()?;
            let command = ViewportCommand::from_u8(code)
                .ok_or(WireError::InvalidData("unknown viewport command"))?;
            Ok(ClientFrame::ViewportCommand { command })
        }
        CLIENT_KIND_UNKNOWN => Ok(ClientFrame::Unknown),
        other => Err(WireError::UnknownFrameType(other)),
    }
}

fn encode_updates(buf: &mut Vec<u8>, updates: &[Update]) {
    write_var_u32(buf, updates.len() as u32);
    for update in updates {
        match update {
            Update::Cell {
                row,
                col,
                seq,
                cell,
            } => {
                buf.push(UPDATE_KIND_CELL);
                write_var_u32(buf, *row);
                write_var_u32(buf, *col);
                write_var_u64(buf, *seq);
                write_var_u64(buf, *cell);
            }
            Update::Rect {
                rows,
                cols,
                seq,
                cell,
            } => {
                buf.push(UPDATE_KIND_RECT);
                write_var_u32(buf, rows[0]);
                write_var_u32(buf, rows[1]);
                write_var_u32(buf, cols[0]);
                write_var_u32(buf, cols[1]);
                write_var_u64(buf, *seq);
                write_var_u64(buf, *cell);
            }
            Update::Row { row, seq, cells } => {
                buf.push(UPDATE_KIND_ROW);
                write_var_u32(buf, *row);
                write_var_u64(buf, *seq);
                write_var_u32(buf, cells.len() as u32);
                for cell in cells {
                    write_var_u64(buf, *cell);
                }
            }
            Update::RowSegment {
                row,
                start_col,
                seq,
                cells,
            } => {
                buf.push(UPDATE_KIND_SEGMENT);
                write_var_u32(buf, *row);
                write_var_u32(buf, *start_col);
                write_var_u64(buf, *seq);
                write_var_u32(buf, cells.len() as u32);
                for cell in cells {
                    write_var_u64(buf, *cell);
                }
            }
            Update::Trim { start, count, seq } => {
                buf.push(UPDATE_KIND_TRIM);
                write_var_u32(buf, *start);
                write_var_u32(buf, *count);
                write_var_u64(buf, *seq);
            }
            Update::Style {
                id,
                seq,
                fg,
                bg,
                attrs,
            } => {
                buf.push(UPDATE_KIND_STYLE);
                write_var_u32(buf, *id);
                write_var_u64(buf, *seq);
                write_var_u32(buf, *fg);
                write_var_u32(buf, *bg);
                buf.push(*attrs);
            }
        }
    }
}

fn encode_cursor(buf: &mut Vec<u8>, cursor: &CursorFrame) {
    write_var_u32(buf, cursor.row);
    write_var_u32(buf, cursor.col);
    write_var_u64(buf, cursor.seq);
    buf.push(cursor.visible as u8);
    buf.push(cursor.blink as u8);
}

fn decode_updates(cursor: &mut Cursor<'_>) -> Result<Vec<Update>, WireError> {
    let count = cursor.read_var_u32()? as usize;
    let mut updates = Vec::with_capacity(count);
    for _ in 0..count {
        let tag = cursor.read_u8()?;
        let update = match tag {
            UPDATE_KIND_CELL => {
                let row = cursor.read_var_u32()?;
                let col = cursor.read_var_u32()?;
                let seq = cursor.read_var_u64()?;
                let cell = cursor.read_var_u64()?;
                Update::Cell {
                    row,
                    col,
                    seq,
                    cell,
                }
            }
            UPDATE_KIND_RECT => {
                let row_start = cursor.read_var_u32()?;
                let row_end = cursor.read_var_u32()?;
                let col_start = cursor.read_var_u32()?;
                let col_end = cursor.read_var_u32()?;
                let seq = cursor.read_var_u64()?;
                let cell = cursor.read_var_u64()?;
                Update::Rect {
                    rows: [row_start, row_end],
                    cols: [col_start, col_end],
                    seq,
                    cell,
                }
            }
            UPDATE_KIND_ROW => {
                let row = cursor.read_var_u32()?;
                let seq = cursor.read_var_u64()?;
                let len = cursor.read_var_u32()? as usize;
                let mut cells = Vec::with_capacity(len);
                for _ in 0..len {
                    cells.push(cursor.read_var_u64()?);
                }
                Update::Row { row, seq, cells }
            }
            UPDATE_KIND_SEGMENT => {
                let row = cursor.read_var_u32()?;
                let start_col = cursor.read_var_u32()?;
                let seq = cursor.read_var_u64()?;
                let len = cursor.read_var_u32()? as usize;
                let mut cells = Vec::with_capacity(len);
                for _ in 0..len {
                    cells.push(cursor.read_var_u64()?);
                }
                Update::RowSegment {
                    row,
                    start_col,
                    seq,
                    cells,
                }
            }
            UPDATE_KIND_TRIM => {
                let start = cursor.read_var_u32()?;
                let count = cursor.read_var_u32()?;
                let seq = cursor.read_var_u64()?;
                Update::Trim { start, count, seq }
            }
            UPDATE_KIND_STYLE => {
                let id = cursor.read_var_u32()?;
                let seq = cursor.read_var_u64()?;
                let fg = cursor.read_var_u32()?;
                let bg = cursor.read_var_u32()?;
                let attrs = cursor.read_u8()?;
                Update::Style {
                    id,
                    seq,
                    fg,
                    bg,
                    attrs,
                }
            }
            other => return Err(WireError::UnknownUpdateTag(other)),
        };
        updates.push(update);
    }
    Ok(updates)
}

fn decode_cursor(cursor: &mut Cursor<'_>) -> Result<CursorFrame, WireError> {
    let row = cursor.read_var_u32()?;
    let col = cursor.read_var_u32()?;
    let seq = cursor.read_var_u64()?;
    let visible = cursor.read_bool()?;
    let blink = cursor.read_bool()?;
    Ok(CursorFrame {
        row,
        col,
        seq,
        visible,
        blink,
    })
}

fn encode_sync_config(buf: &mut Vec<u8>, config: &SyncConfigFrame) {
    write_var_u32(buf, config.snapshot_budgets.len() as u32);
    for LaneBudgetFrame { lane, max_updates } in &config.snapshot_budgets {
        buf.push(lane.as_u8());
        write_var_u32(buf, *max_updates);
    }
    write_var_u32(buf, config.delta_budget);
    write_var_u64(buf, config.heartbeat_ms);
    write_var_u32(buf, config.initial_snapshot_lines);
}

fn decode_sync_config(cursor: &mut Cursor<'_>) -> Result<SyncConfigFrame, WireError> {
    let count = cursor.read_var_u32()? as usize;
    let mut budgets = Vec::with_capacity(count);
    for _ in 0..count {
        let lane = decode_lane(cursor)?;
        let max_updates = cursor.read_var_u32()?;
        budgets.push(LaneBudgetFrame { lane, max_updates });
    }
    let delta_budget = cursor.read_var_u32()?;
    let heartbeat_ms = cursor.read_var_u64()?;
    let initial_snapshot_lines = cursor.read_var_u32()?;
    Ok(SyncConfigFrame {
        snapshot_budgets: budgets,
        delta_budget,
        heartbeat_ms,
        initial_snapshot_lines,
    })
}

fn write_header(buf: &mut Vec<u8>, kind: u8) {
    let version = PROTOCOL_VERSION & ((1 << VERSION_BITS) - 1);
    buf.push((version << 5) | (kind & TYPE_MASK));
}

fn read_header(cursor: &mut Cursor<'_>) -> Result<(u8, u8), WireError> {
    let byte = cursor.read_u8()?;
    let version = (byte & VERSION_MASK) >> 5;
    let kind = byte & TYPE_MASK;
    if version != (PROTOCOL_VERSION & ((1 << VERSION_BITS) - 1)) {
        return Err(WireError::InvalidVersion(version));
    }
    Ok((kind, version))
}

fn write_var_u32(buf: &mut Vec<u8>, value: u32) {
    write_var_u64(buf, value as u64);
}

fn write_var_u64(buf: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        buf.push((value as u8) | 0x80);
        value >>= 7;
    }
    buf.push(value as u8);
}

#[derive(Clone, Copy)]
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn read_u8(&mut self) -> Result<u8, WireError> {
        if self.pos >= self.bytes.len() {
            return Err(WireError::UnexpectedEof);
        }
        let value = self.bytes[self.pos];
        self.pos += 1;
        Ok(value)
    }

    fn read_var_u64(&mut self) -> Result<u64, WireError> {
        let mut result: u64 = 0;
        let mut shift = 0;
        while shift < 64 {
            let byte = self.read_u8()?;
            result |= ((byte & 0x7F) as u64) << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
        }
        Err(WireError::VarIntOverflow)
    }

    fn read_var_u32(&mut self) -> Result<u32, WireError> {
        let value = self.read_var_u64()?;
        if value > u32::MAX as u64 {
            return Err(WireError::InvalidData("u32 overflow"));
        }
        Ok(value as u32)
    }

    fn read_bool(&mut self) -> Result<bool, WireError> {
        match self.read_u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(WireError::InvalidData("invalid boolean")),
        }
    }

    fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], WireError> {
        if self.pos + len > self.bytes.len() {
            return Err(WireError::UnexpectedEof);
        }
        let slice = &self.bytes[self.pos..self.pos + len];
        self.pos += len;
        Ok(slice)
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }
}

fn decode_lane(cursor: &mut Cursor<'_>) -> Result<Lane, WireError> {
    match cursor.read_u8()? {
        0 => Ok(Lane::Foreground),
        1 => Ok(Lane::Recent),
        2 => Ok(Lane::History),
        _ => Err(WireError::InvalidData("invalid lane")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_timeout::timeout]
    fn encode_decode_heartbeat() {
        let frame = HostFrame::Heartbeat {
            seq: 42,
            timestamp_ms: 1234,
        };
        let encoded = encode_host_frame_binary(&frame);
        let decoded = decode_host_frame_binary(&encoded).expect("decode");
        assert_eq!(frame, decoded);
    }

    #[test_timeout::timeout]
    fn encode_decode_hello() {
        let frame = HostFrame::Hello {
            subscription: 7,
            max_seq: 9000,
            config: SyncConfigFrame {
                snapshot_budgets: vec![
                    LaneBudgetFrame {
                        lane: Lane::Foreground,
                        max_updates: 8,
                    },
                    LaneBudgetFrame {
                        lane: Lane::History,
                        max_updates: 16,
                    },
                ],
                delta_budget: 128,
                heartbeat_ms: 250,
                initial_snapshot_lines: 8,
            },
            features: 0,
        };
        let encoded = encode_host_frame_binary(&frame);
        let decoded = decode_host_frame_binary(&encoded).expect("decode");
        assert_eq!(frame, decoded);
    }

    #[test_timeout::timeout]
    fn encode_decode_snapshot_with_updates() {
        let frame = HostFrame::Snapshot {
            subscription: 1,
            lane: Lane::Foreground,
            watermark: 55,
            has_more: true,
            updates: vec![
                Update::Cell {
                    row: 3,
                    col: 4,
                    seq: 10,
                    cell: 0xDEADBEEF,
                },
                Update::Row {
                    row: 5,
                    seq: 12,
                    cells: vec![0, 1, 2],
                },
                Update::RowSegment {
                    row: 6,
                    start_col: 2,
                    seq: 13,
                    cells: vec![9, 9, 9, 9],
                },
                Update::Style {
                    id: 7,
                    seq: 14,
                    fg: 0x010203,
                    bg: 0x040506,
                    attrs: 0b10101010,
                },
                Update::Trim {
                    start: 1,
                    count: 2,
                    seq: 15,
                },
            ],
            cursor: Some(CursorFrame {
                row: 6,
                col: 6,
                seq: 16,
                visible: true,
                blink: false,
            }),
        };
        let encoded = encode_host_frame_binary(&frame);
        let decoded = decode_host_frame_binary(&encoded).expect("decode");
        assert_eq!(frame, decoded);
    }

    #[test_timeout::timeout]
    fn encode_decode_history_backfill() {
        let frame = HostFrame::HistoryBackfill {
            subscription: 4,
            request_id: 2,
            start_row: 5,
            count: 2,
            updates: vec![Update::Cell {
                row: 5,
                col: 0,
                seq: 1,
                cell: 0x0002,
            }],
            more: true,
            cursor: Some(CursorFrame {
                row: 5,
                col: 1,
                seq: 3,
                visible: false,
                blink: true,
            }),
        };
        let encoded = encode_host_frame_binary(&frame);
        let decoded = decode_host_frame_binary(&encoded).expect("decode");
        assert_eq!(frame, decoded);
    }

    #[test_timeout::timeout]
    fn encode_decode_delta_no_updates() {
        let frame = HostFrame::Delta {
            subscription: 3,
            watermark: 10,
            has_more: false,
            updates: Vec::new(),
            cursor: Some(CursorFrame {
                row: 9,
                col: 0,
                seq: 11,
                visible: true,
                blink: true,
            }),
        };
        let encoded = encode_host_frame_binary(&frame);
        let decoded = decode_host_frame_binary(&encoded).expect("decode");
        assert_eq!(frame, decoded);
    }

    #[test_timeout::timeout]
    fn encode_decode_client_frames() {
        let input = ClientFrame::Input {
            seq: 99,
            data: vec![1, 2, 3, 4],
        };
        let resize = ClientFrame::Resize { cols: 80, rows: 24 };
        let viewport = ClientFrame::ViewportCommand {
            command: ViewportCommand::Clear,
        };

        let encoded_input = encode_client_frame_binary(&input);
        let decoded_input = decode_client_frame_binary(&encoded_input).expect("decode input");
        assert_eq!(input, decoded_input);

        let encoded_resize = encode_client_frame_binary(&resize);
        let decoded_resize = decode_client_frame_binary(&encoded_resize).expect("decode resize");
        assert_eq!(resize, decoded_resize);

        let encoded_viewport = encode_client_frame_binary(&viewport);
        let decoded_viewport =
            decode_client_frame_binary(&encoded_viewport).expect("decode viewport");
        assert_eq!(viewport, decoded_viewport);
    }

    #[test_timeout::timeout]
    fn env_toggle_respects_flag() {
        assert!(parse_flag("true"));
        assert!(parse_flag("YES"));
        assert!(parse_flag("1"));
        assert!(!parse_flag("false"));
        assert!(!parse_flag("0"));
        assert!(!parse_flag(""));
    }

    #[test_timeout::timeout]
    fn encode_decode_cursor_frame() {
        let frame = HostFrame::Cursor {
            subscription: 2,
            cursor: CursorFrame {
                row: 7,
                col: 3,
                seq: 5,
                visible: false,
                blink: false,
            },
        };
        let encoded = encode_host_frame_binary(&frame);
        let decoded = decode_host_frame_binary(&encoded).expect("decode cursor");
        assert_eq!(frame, decoded);
    }
}

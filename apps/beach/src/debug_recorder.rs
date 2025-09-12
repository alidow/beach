use std::fs::File;
use std::io::{BufWriter, Write};
use serde::Serialize;
use chrono::{DateTime, Utc};
use anyhow::Result;

use crate::protocol::{ServerMessage, ClientMessage};
use crate::server::terminal_state::Grid;

#[derive(Serialize)]
#[serde(tag = "type")]
pub enum DebugEvent {
    #[serde(rename = "client_message")]
    ClientMessage {
        timestamp: DateTime<Utc>,
        message: ClientMessage,
    },
    
    #[serde(rename = "server_message")]
    ServerMessage {
        timestamp: DateTime<Utc>,
        message: ServerMessage,
    },
    
    #[serde(rename = "client_grid_state")]
    ClientGridState {
        timestamp: DateTime<Utc>,
        grid: Grid,
        scroll_offset: i64,
        view_mode: String,
    },
    
    #[serde(rename = "server_backend_state")]
    ServerBackendState {
        timestamp: DateTime<Utc>,
        grid: Grid,
        cursor_pos: (u16, u16),
    },
    
    #[serde(rename = "server_subscription_view")]
    ServerSubscriptionView {
        timestamp: DateTime<Utc>,
        subscription_id: String,
        grid: Grid,
        view_mode: String,
    },
    
    #[serde(rename = "server_subscription_snapshot")]
    ServerSubscriptionSnapshot {
        timestamp: DateTime<Utc>,
        subscription_id: String,
        sequence: u64,
        /// Grid dimensions
        dimensions: (u16, u16),
        /// Count of non-blank lines
        non_blank_lines: usize,
        /// Count of blank lines
        blank_line_count: usize,
        /// Sample of content (first few non-blank lines)
        content_sample: Vec<String>,
        /// Cursor position and visibility
        cursor_info: Option<(u16, u16, bool)>, // (row, col, visible)
    },
    
    #[serde(rename = "snapshot_transformation")]
    SnapshotTransformation {
        timestamp: DateTime<Utc>,
        stage: String,
        dimensions: (u16, u16),
        blank_lines: Vec<u16>,
        content_sample: Vec<String>,
    },
    
    #[serde(rename = "server_subscription_delta")]
    ServerSubscriptionDelta {
        timestamp: DateTime<Utc>,
        subscription_id: String,
        sequence: u64,
        /// Number of cell changes
        cell_changes_count: usize,
        /// Has dimension change
        has_dimension_change: bool,
        /// Has cursor change
        has_cursor_change: bool,
        /// Lines that were modified
        modified_lines: Vec<u16>,
    },
    
    #[serde(rename = "client_delta_application")]
    ClientDeltaApplication {
        timestamp: DateTime<Utc>,
        /// Sequence number of the delta
        sequence: u64,
        /// Number of cell changes to apply
        cell_changes_count: usize,
        /// Lines that will be modified
        modified_lines: Vec<u16>,
        /// Has dimension change
        has_dimension_change: bool,
        /// Grid dimensions before applying delta
        before_dims: (u16, u16),
        /// Grid dimensions after applying delta (if changed)
        after_dims: Option<(u16, u16)>,
        /// Count of blank lines before
        blank_lines_before: usize,
        /// Count of blank lines after
        blank_lines_after: usize,
        /// Sample of affected lines before change
        lines_before: Vec<String>,
        /// Sample of affected lines after change
        lines_after: Vec<String>,
    },
    
    #[serde(rename = "server_pty_output")]
    ServerPtyOutput {
        timestamp: DateTime<Utc>,
        /// Raw bytes from PTY
        bytes: Vec<u8>,
        /// Human-readable representation with escape sequences visible
        readable: String,
    },
    
    #[serde(rename = "server_alacritty_state")]
    ServerAlacrittyState {
        timestamp: DateTime<Utc>,
        /// Grid dimensions
        dimensions: (u16, u16),
        /// Sample of grid content around interesting areas
        content_sample: Vec<String>,
        /// Count of blank lines in the grid
        blank_line_count: usize,
    },
    
    #[serde(rename = "process_output_call")]
    ProcessOutputCall {
        timestamp: DateTime<Utc>,
        /// Sequence number for this call
        sequence: u64,
        /// Length of input data
        data_len: usize,
        /// Hash of the data
        data_hash: u64,
        /// Human-readable preview
        preview: String,
    },
    
    #[serde(rename = "newline_conversion")]
    NewlineConversion {
        timestamp: DateTime<Utc>,
        /// Original bytes
        original: Vec<u8>,
        /// Converted bytes
        converted: Vec<u8>,
        /// Number of conversions made
        conversions_count: usize,
    },
    
    #[serde(rename = "grid_before_after")]
    GridBeforeAfter {
        timestamp: DateTime<Utc>,
        /// Grid dimensions before
        before_dims: (u16, u16),
        /// Grid dimensions after
        after_dims: (u16, u16),
        /// Lines that changed
        changed_lines: Vec<u16>,
        /// New blank lines added
        new_blank_lines: Vec<u16>,
    },
    
    #[serde(rename = "pty_read_chunk")]
    PtyReadChunk {
        timestamp: DateTime<Utc>,
        /// Sequence number for PTY reads
        sequence: u64,
        /// Size of chunk
        chunk_size: usize,
        /// Hash of chunk
        chunk_hash: u64,
        /// Whether this hash was seen before
        is_duplicate: bool,
    },
    
    #[serde(rename = "alacritty_vs_gridhistory")]
    AlacrittyVsGridHistory {
        timestamp: DateTime<Utc>,
        /// Alacritty grid dimensions
        alacritty_dims: (u16, u16),
        /// GridHistory grid dimensions
        gridhistory_dims: (u16, u16),
        /// Alacritty blank line count
        alacritty_blank_lines: usize,
        /// GridHistory blank line count
        gridhistory_blank_lines: usize,
        /// Lines that differ between the two
        differing_lines: Vec<u16>,
        /// Sample of differences (first 5)
        difference_samples: Vec<String>,
    },
    
    #[serde(rename = "alacritty_grid_dump")]
    AlacrittyGridDump {
        timestamp: DateTime<Utc>,
        /// Grid dimensions
        dimensions: (u16, u16),
        /// All non-blank lines with their content
        non_blank_lines: Vec<(u16, String)>,
        /// Total blank line count
        blank_line_count: usize,
    },
    
    #[serde(rename = "comment")]
    Comment {
        timestamp: DateTime<Utc>,
        text: String,
    },
    
    #[serde(rename = "grid_delta_application")]
    GridDeltaApplication {
        timestamp: DateTime<Utc>,
        /// Context where delta is being applied
        context: String,
        /// Sequence number of the delta
        sequence: u64,
        /// Number of cell changes to apply
        cell_changes_count: usize,
        /// Lines that will be modified
        modified_lines: Vec<u16>,
        /// Has dimension change
        has_dimension_change: bool,
        /// Dimension change details if any
        dimension_change: Option<(u16, u16, u16, u16)>, // old_width, old_height, new_width, new_height
        /// Has cursor change
        has_cursor_change: bool,
        /// Grid dimensions before applying delta
        before_dims: (u16, u16),
        /// Grid dimensions after applying delta
        after_dims: (u16, u16),
        /// Count of blank lines before
        blank_lines_before: usize,
        /// Count of blank lines after
        blank_lines_after: usize,
        /// Sample of content before (first few lines)
        content_before: Vec<String>,
        /// Sample of content after (first few lines)
        content_after: Vec<String>,
    },
    
    #[serde(rename = "snapshot_comparison")]
    SnapshotComparison {
        timestamp: DateTime<Utc>,
        /// Context where comparison is happening
        context: String,
        /// Dimensions of backend grid
        backend_dims: (u16, u16),
        /// Dimensions of history grid
        history_dims: (u16, u16),
        /// Blank lines in backend grid
        backend_blank_lines: usize,
        /// Blank lines in history grid
        history_blank_lines: usize,
        /// Content distribution in backend (row_index, has_content)
        backend_content_dist: Vec<(u16, bool)>,
        /// Content distribution in history (row_index, has_content)
        history_content_dist: Vec<(u16, bool)>,
        /// Lines that differ between backend and history
        differing_lines: Vec<u16>,
        /// Sample of actual differences (row, backend_content, history_content)
        difference_samples: Vec<(u16, String, String)>,
    },

    #[serde(rename = "grid_bottom_context")]
    GridBottomContext {
        timestamp: DateTime<Utc>,
        /// Context label (where this was captured)
        context: String,
        /// Grid dimensions
        dimensions: (u16, u16),
        /// Count of trailing blank rows at bottom of grid
        trailing_blank_count: usize,
        /// Last N rows (row index, trimmed text)
        last_rows: Vec<(u16, String)>,
    },

    #[serde(rename = "server_delta_context")]
    ServerDeltaContext {
        timestamp: DateTime<Utc>,
        subscription_id: String,
        sequence: u64,
        window_start: u16,
        before_lines: Vec<(u16, String)>,
        after_lines: Vec<(u16, String)>,
    },

    #[serde(rename = "client_seam_context")]
    ClientSeamContext {
        timestamp: DateTime<Utc>,
        sequence: u64,
        window_start: u16,
        before_lines: Vec<(u16, String)>,
        after_lines: Vec<(u16, String)>,
    },
    
    // Scrollback debugging events
    #[serde(rename = "history_lookup_requested")]
    HistoryLookupRequested {
        timestamp: DateTime<Utc>,
        requested_line: u64,
    },
    
    #[serde(rename = "history_lookup_candidate")]
    HistoryLookupCandidate {
        timestamp: DateTime<Utc>,
        snapshot_index: usize,
        snapshot_start_line: u64,
        snapshot_end_line: u64,
        contains_line: bool,
    },
    
    #[serde(rename = "history_reconstruct_end")]
    HistoryReconstructEnd {
        timestamp: DateTime<Utc>,
        target_line: u64,
        found_snapshot: bool,
        result_start_line: Option<u64>,
        result_end_line: Option<u64>,
        result_blank_count: Option<usize>,
    },
    
    #[serde(rename = "reconstruction_path")]
    ReconstructionPath {
        timestamp: DateTime<Utc>,
        starting_snapshot_index: usize,
        starting_line: u64,
        deltas_applied: usize,
        final_line: u64,
    },
    
    #[serde(rename = "historical_view_requested")]
    HistoricalViewRequested {
        timestamp: DateTime<Utc>,
        requested_line: u64,
        height: u16,
    },
    
    #[serde(rename = "historical_view_returned")]
    HistoricalViewReturned {
        timestamp: DateTime<Utc>,
        requested_line: u64,
        returned_start_line: u64,
        returned_end_line: u64,
        blank_lines: usize,
        sample_content: Vec<String>,
    },
    
    #[serde(rename = "snapshot_with_view_request")]
    SnapshotWithViewRequest {
        timestamp: DateTime<Utc>,
        mode: String,
        position_line: Option<u64>,
        position_time: Option<i64>,
        dimensions: (u16, u16),
    },
    
    #[serde(rename = "snapshot_with_view_response")]
    SnapshotWithViewResponse {
        timestamp: DateTime<Utc>,
        mode: String,
        result_start_line: u64,
        result_end_line: u64,
        result_blank_count: usize,
        sample_content: Vec<String>,
    },
    
    #[serde(rename = "modify_subscription_received")]
    ModifySubscriptionReceived {
        timestamp: DateTime<Utc>,
        subscription_id: String,
        mode: String,
        position: Option<String>,
    },
    
    // Client scrollback events
    #[serde(rename = "client_scroll_event")]
    ClientScrollEvent {
        timestamp: DateTime<Utc>,
        /// Direction: "up" or "down"
        direction: String,
        /// Current scroll offset
        scroll_offset: usize,
        /// View line if in historical mode
        view_line: Option<u64>,
    },
    
    #[serde(rename = "client_history_needs_check")]
    ClientHistoryNeedsCheck {
        timestamp: DateTime<Utc>,
        /// Current scroll offset
        scroll_offset: usize,
        /// Whether metadata is available
        has_metadata: bool,
        /// Current view line
        view_line: Option<u64>,
        /// History request generated
        request: Option<(u64, u64)>,
    },
    
    #[serde(rename = "client_history_request_sent")]
    ClientHistoryRequestSent {
        timestamp: DateTime<Utc>,
        /// View mode being requested
        mode: String,
        /// Start line for historical request
        start_line: Option<u64>,
        /// End line for historical request
        end_line: Option<u64>,
    },
    
    #[serde(rename = "modify_subscription_processed")]
    ModifySubscriptionProcessed {
        timestamp: DateTime<Utc>,
        subscription_id: String,
        mode: String,
        result_grid_dims: (u16, u16),
        result_blank_count: usize,
    },
}

#[derive(Debug)]
pub struct DebugRecorder {
    writer: BufWriter<File>,
}

impl DebugRecorder {
    pub fn new(path: &str) -> Result<Self> {
        let file = File::create(path)?;
        let writer = BufWriter::new(file);
        Ok(Self { writer })
    }
    
    pub fn record_grid_bottom_context(&mut self, context: &str, grid: &Grid, last_n: u16) -> Result<()> {
        // Compute trailing blank rows
        let mut trailing_blank_count = 0usize;
        for row in (0..grid.height).rev() {
            let mut line = String::new();
            for col in 0..grid.width {
                if let Some(cell) = grid.get_cell(row, col) {
                    line.push(cell.char);
                } else {
                    line.push(' ');
                }
            }
            let trimmed = line.trim_end();
            let is_blank = trimmed.is_empty();
            if is_blank {
                trailing_blank_count += 1;
            } else {
                break;
            }
        }

        // Collect last N rows (trimmed)
        let mut last_rows = Vec::new();
        let start = grid.height.saturating_sub(last_n);
        for row in start..grid.height {
            let mut line = String::new();
            for col in 0..grid.width {
                if let Some(cell) = grid.get_cell(row, col) {
                    line.push(cell.char);
                } else {
                    line.push(' ');
                }
            }
            last_rows.push((row, line.trim_end().to_string()));
        }

        self.record_event(DebugEvent::GridBottomContext {
            timestamp: Utc::now(),
            context: context.to_string(),
            dimensions: (grid.width, grid.height),
            trailing_blank_count,
            last_rows,
        })
    }

    pub fn record_server_delta_context(
        &mut self,
        subscription_id: &str,
        sequence: u64,
        window_start: u16,
        before_lines: &[(u16, String)],
        after_lines: &[(u16, String)],
    ) -> Result<()> {
        self.record_event(DebugEvent::ServerDeltaContext {
            timestamp: Utc::now(),
            subscription_id: subscription_id.to_string(),
            sequence,
            window_start,
            before_lines: before_lines.to_vec(),
            after_lines: after_lines.to_vec(),
        })
    }

    pub fn record_client_seam_context(
        &mut self,
        sequence: u64,
        window_start: u16,
        before_lines: &[(u16, String)],
        after_lines: &[(u16, String)],
    ) -> Result<()> {
        self.record_event(DebugEvent::ClientSeamContext {
            timestamp: Utc::now(),
            sequence,
            window_start,
            before_lines: before_lines.to_vec(),
            after_lines: after_lines.to_vec(),
        })
    }

    pub fn record_event(&mut self, event: DebugEvent) -> Result<()> {
        let json = serde_json::to_string(&event)?;
        writeln!(self.writer, "{}", json)?;
        self.writer.flush()?;
        Ok(())
    }
    
    pub fn record_client_message(&mut self, msg: &ClientMessage) -> Result<()> {
        self.record_event(DebugEvent::ClientMessage {
            timestamp: Utc::now(),
            message: msg.clone(),
        })
    }
    
    pub fn record_server_message(&mut self, msg: &ServerMessage) -> Result<()> {
        self.record_event(DebugEvent::ServerMessage {
            timestamp: Utc::now(),
            message: msg.clone(),
        })
    }
    
    pub fn record_client_grid_state(&mut self, grid: &Grid, scroll_offset: i64, view_mode: &str) -> Result<()> {
        self.record_event(DebugEvent::ClientGridState {
            timestamp: Utc::now(),
            grid: grid.clone(),
            scroll_offset,
            view_mode: view_mode.to_string(),
        })
    }
    
    pub fn record_server_backend_state(&mut self, grid: &Grid, cursor_pos: (u16, u16)) -> Result<()> {
        self.record_event(DebugEvent::ServerBackendState {
            timestamp: Utc::now(),
            grid: grid.clone(),
            cursor_pos,
        })
    }
    
    pub fn record_server_subscription_view(&mut self, subscription_id: &str, grid: &Grid, view_mode: &str) -> Result<()> {
        self.record_event(DebugEvent::ServerSubscriptionView {
            timestamp: Utc::now(),
            subscription_id: subscription_id.to_string(),
            grid: grid.clone(),
            view_mode: view_mode.to_string(),
        })
    }
    
    pub fn record_pty_output(&mut self, bytes: &[u8]) -> Result<()> {
        // Create readable representation showing escape sequences
        let mut readable = String::new();
        for &byte in bytes {
            match byte {
                0x1B => readable.push_str("\\x1B"),  // ESC
                b'\n' => readable.push_str("\\n"),
                b'\r' => readable.push_str("\\r"),
                b'\t' => readable.push_str("\\t"),
                0x00..=0x1F | 0x7F => readable.push_str(&format!("\\x{:02x}", byte)),
                _ => readable.push(byte as char),
            }
        }
        
        self.record_event(DebugEvent::ServerPtyOutput {
            timestamp: Utc::now(),
            bytes: bytes.to_vec(),
            readable,
        })
    }
    
    pub fn record_alacritty_state(&mut self, dimensions: (u16, u16), content_sample: Vec<String>, blank_count: usize) -> Result<()> {
        self.record_event(DebugEvent::ServerAlacrittyState {
            timestamp: Utc::now(),
            dimensions,
            content_sample,
            blank_line_count: blank_count,
        })
    }
    
    pub fn record_comment(&mut self, text: &str) -> Result<()> {
        self.record_event(DebugEvent::Comment {
            timestamp: Utc::now(),
            text: text.to_string(),
        })
    }
    
    pub fn record_process_output_call(&mut self, sequence: u64, data: &[u8]) -> Result<()> {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        data.hash(&mut hasher);
        let hash = hasher.finish();
        
        // Create preview (first 50 chars)
        let mut preview = String::new();
        for &byte in data.iter().take(50) {
            match byte {
                0x1B => preview.push_str("\\x1B"),
                b'\n' => preview.push_str("\\n"),
                b'\r' => preview.push_str("\\r"),
                b'\t' => preview.push_str("\\t"),
                0x00..=0x1F | 0x7F => preview.push_str(&format!("\\x{:02x}", byte)),
                _ => preview.push(byte as char),
            }
        }
        if data.len() > 50 {
            preview.push_str("...");
        }
        
        self.record_event(DebugEvent::ProcessOutputCall {
            timestamp: Utc::now(),
            sequence,
            data_len: data.len(),
            data_hash: hash,
            preview,
        })
    }
    
    pub fn record_newline_conversion(&mut self, original: &[u8], converted: &[u8], conversions: usize) -> Result<()> {
        self.record_event(DebugEvent::NewlineConversion {
            timestamp: Utc::now(),
            original: original.to_vec(),
            converted: converted.to_vec(),
            conversions_count: conversions,
        })
    }
    
    pub fn record_grid_before_after(&mut self, before: &crate::server::terminal_state::Grid, after: &crate::server::terminal_state::Grid) -> Result<()> {
        let mut changed_lines = Vec::new();
        let mut new_blank_lines = Vec::new();
        
        for row in 0..after.height.min(before.height) {
            let before_empty = (0..before.width).all(|col| {
                before.get_cell(row, col).map(|c| c.char == ' ' || c.char == '\0').unwrap_or(true)
            });
            let after_empty = (0..after.width).all(|col| {
                after.get_cell(row, col).map(|c| c.char == ' ' || c.char == '\0').unwrap_or(true)
            });
            
            // Check if line changed
            let changed = (0..before.width.min(after.width)).any(|col| {
                let before_cell = before.get_cell(row, col);
                let after_cell = after.get_cell(row, col);
                before_cell != after_cell
            });
            
            if changed {
                changed_lines.push(row);
            }
            if !before_empty && after_empty {
                new_blank_lines.push(row);
            }
        }
        
        self.record_event(DebugEvent::GridBeforeAfter {
            timestamp: Utc::now(),
            before_dims: (before.width, before.height),
            after_dims: (after.width, after.height),
            changed_lines,
            new_blank_lines,
        })
    }
    
    pub fn record_pty_read_chunk(&mut self, sequence: u64, data: &[u8], is_duplicate: bool) -> Result<()> {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        data.hash(&mut hasher);
        let hash = hasher.finish();
        
        self.record_event(DebugEvent::PtyReadChunk {
            timestamp: Utc::now(),
            sequence,
            chunk_size: data.len(),
            chunk_hash: hash,
            is_duplicate,
        })
    }
    
    pub fn record_snapshot_transformation(&mut self,
        stage: &str,
        grid: &Grid
    ) -> Result<()> {
        let mut blank_lines = Vec::new();
        let mut content_sample = Vec::new();
        
        for row in 0..grid.height.min(10) {
            let line_content: String = (0..grid.width).map(|col| {
                grid.get_cell(row, col)
                    .map(|c| c.char)
                    .unwrap_or(' ')
            }).collect();
            
            let trimmed = line_content.trim_end();
            if trimmed.is_empty() {
                blank_lines.push(row);
                content_sample.push(format!("Row {}: [BLANK]", row));
            } else {
                content_sample.push(format!("Row {}: {}", row, trimmed));
            }
        }
        
        self.record_event(DebugEvent::SnapshotTransformation {
            timestamp: Utc::now(),
            stage: stage.to_string(),
            dimensions: (grid.width, grid.height),
            blank_lines,
            content_sample,
        })
    }
    
    pub fn record_server_subscription_snapshot(&mut self, 
        subscription_id: &str,
        sequence: u64,
        grid: &Grid
    ) -> Result<()> {
        let mut non_blank_lines = Vec::new();
        let mut blank_count = 0;
        let mut content_sample = Vec::new();
        
        for row in 0..grid.height {
            let line_content: String = (0..grid.width).map(|col| {
                grid.get_cell(row, col)
                    .map(|c| c.char)
                    .unwrap_or(' ')
            }).collect();
            
            let trimmed = line_content.trim_end();
            if trimmed.is_empty() {
                blank_count += 1;
                // For debugging blank line issue, show blank lines near content
                if row > 0 && row < grid.height - 1 {
                    // Check if there's content nearby
                    let prev_line: String = (0..grid.width).map(|col| {
                        grid.get_cell(row - 1, col)
                            .map(|c| c.char)
                            .unwrap_or(' ')
                    }).collect();
                    let next_line: String = (0..grid.width).map(|col| {
                        grid.get_cell(row + 1, col)
                            .map(|c| c.char)
                            .unwrap_or(' ')
                    }).collect();
                    
                    if !prev_line.trim_end().is_empty() || !next_line.trim_end().is_empty() {
                        // This blank line is between content
                        if content_sample.len() < 20 {
                            content_sample.push(format!("Row {}: [BLANK]", row));
                        }
                    }
                }
            } else {
                non_blank_lines.push((row, trimmed.to_string()));
                if content_sample.len() < 20 {
                    content_sample.push(format!("Row {}: {}", row, trimmed));
                }
            }
        }
        
        self.record_event(DebugEvent::ServerSubscriptionSnapshot {
            timestamp: Utc::now(),
            subscription_id: subscription_id.to_string(),
            sequence,
            dimensions: (grid.width, grid.height),
            non_blank_lines: non_blank_lines.len(),
            blank_line_count: blank_count,
            content_sample,
            cursor_info: Some((grid.cursor.row, grid.cursor.col, grid.cursor.visible)),
        })
    }
    
    pub fn record_alacritty_grid_dump(&mut self, grid: &Grid) -> Result<()> {
        let mut non_blank_lines = Vec::new();
        let mut blank_count = 0;
        
        for row in 0..grid.height {
            let line_content: String = (0..grid.width).map(|col| {
                grid.get_cell(row, col)
                    .map(|c| c.char)
                    .unwrap_or(' ')
            }).collect();
            
            let trimmed = line_content.trim_end();
            if trimmed.is_empty() {
                blank_count += 1;
            } else {
                non_blank_lines.push((row, trimmed.to_string()));
            }
        }
        
        self.record_event(DebugEvent::AlacrittyGridDump {
            timestamp: Utc::now(),
            dimensions: (grid.width, grid.height),
            non_blank_lines,
            blank_line_count: blank_count,
        })
    }
    
    pub fn record_alacritty_vs_gridhistory(&mut self, 
        alacritty_grid: &Grid,
        gridhistory_grid: &Grid
    ) -> Result<()> {
        // Count blank lines in each grid
        let alacritty_blank = (0..alacritty_grid.height).filter(|&row| {
            (0..alacritty_grid.width).all(|col| {
                alacritty_grid.get_cell(row, col)
                    .map(|c| c.char == ' ' || c.char == '\0')
                    .unwrap_or(true)
            })
        }).count();
        
        let gridhistory_blank = (0..gridhistory_grid.height).filter(|&row| {
            (0..gridhistory_grid.width).all(|col| {
                gridhistory_grid.get_cell(row, col)
                    .map(|c| c.char == ' ' || c.char == '\0')
                    .unwrap_or(true)
            })
        }).count();
        
        // Find differing lines
        let mut differing_lines = Vec::new();
        let mut difference_samples = Vec::new();
        
        let max_rows = alacritty_grid.height.max(gridhistory_grid.height);
        for row in 0..max_rows {
            let alac_line = if row < alacritty_grid.height {
                (0..alacritty_grid.width).map(|col| {
                    alacritty_grid.get_cell(row, col)
                        .map(|c| c.char)
                        .unwrap_or(' ')
                }).collect::<String>()
            } else {
                String::new()
            };
            
            let gh_line = if row < gridhistory_grid.height {
                (0..gridhistory_grid.width).map(|col| {
                    gridhistory_grid.get_cell(row, col)
                        .map(|c| c.char)
                        .unwrap_or(' ')
                }).collect::<String>()
            } else {
                String::new()
            };
            
            if alac_line.trim_end() != gh_line.trim_end() {
                differing_lines.push(row);
                if difference_samples.len() < 5 {
                    difference_samples.push(format!(
                        "Row {}: Alac='{}' GH='{}'",
                        row,
                        alac_line.trim_end(),
                        gh_line.trim_end()
                    ));
                }
            }
        }
        
        self.record_event(DebugEvent::AlacrittyVsGridHistory {
            timestamp: Utc::now(),
            alacritty_dims: (alacritty_grid.width, alacritty_grid.height),
            gridhistory_dims: (gridhistory_grid.width, gridhistory_grid.height),
            alacritty_blank_lines: alacritty_blank,
            gridhistory_blank_lines: gridhistory_blank,
            differing_lines,
            difference_samples,
        })
    }
}

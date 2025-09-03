use std::sync::{Arc, Mutex};
use chrono::Utc;
use crate::server::terminal_state::{GridView, GridHistory, TerminalStateTracker};

/// Handler for debug requests from the session server
pub struct DebugHandler {
    terminal_tracker: Arc<Mutex<Option<Arc<Mutex<TerminalStateTracker>>>>>,
}

impl DebugHandler {
    pub fn new() -> Self {
        DebugHandler {
            terminal_tracker: Arc::new(Mutex::new(None)),
        }
    }
    
    /// Initialize with a terminal tracker
    pub fn set_tracker(&self, tracker: Arc<Mutex<TerminalStateTracker>>) {
        let mut guard = self.terminal_tracker.lock().unwrap();
        *guard = Some(tracker);
    }
    
    /// Handle a debug request and return a response
    pub async fn handle_debug_request(&self, request: serde_json::Value) -> serde_json::Value {
        // Define DebugRequest locally as it's not accessible from beach-road
        #[derive(Debug, Clone, serde::Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum DebugRequest {
            GetGridView {
                width: Option<u16>,
                height: Option<u16>,
                at_time: Option<chrono::DateTime<Utc>>,
                from_line: Option<u64>,
            },
            GetStats,
            ClearHistory,
        }
        
        // Parse the debug request
        let debug_req = match serde_json::from_value::<DebugRequest>(request) {
            Ok(req) => req,
            Err(e) => {
                return serde_json::json!({
                    "type": "error",
                    "message": format!("Invalid debug request: {}", e)
                });
            }
        };
        
        // Handle different debug request types
        match debug_req {
            DebugRequest::GetGridView { width, height, at_time, from_line } => {
                match self.get_grid_view(width, height, at_time, from_line) {
                    Ok(response) => serde_json::json!({
                        "type": "grid_view",
                        "width": response.width,
                        "height": response.height,
                        "cursor_row": response.cursor_row,
                        "cursor_col": response.cursor_col,
                        "cursor_visible": response.cursor_visible,
                        "rows": response.rows,
                        "ansi_rows": response.ansi_rows,
                        "timestamp": response.timestamp,
                        "start_line": response.start_line,
                        "end_line": response.end_line,
                    }),
                    Err(e) => serde_json::json!({
                        "type": "error",
                        "message": e
                    })
                }
            }
            DebugRequest::GetStats => {
                match self.get_stats() {
                    Ok(stats) => serde_json::json!({
                        "type": "stats",
                        "history_size_bytes": stats.history_size_bytes,
                        "total_deltas": stats.total_deltas,
                        "total_snapshots": stats.total_snapshots,
                        "current_dimensions": stats.current_dimensions,
                        "session_duration_secs": stats.session_duration_secs,
                    }),
                    Err(e) => serde_json::json!({
                        "type": "error",
                        "message": e
                    })
                }
            }
            DebugRequest::ClearHistory => {
                match self.clear_history() {
                    Ok(()) => serde_json::json!({
                        "type": "success",
                        "message": "History cleared"
                    }),
                    Err(e) => serde_json::json!({
                        "type": "error", 
                        "message": e
                    })
                }
            }
        }
    }
    
    /// Get the current grid view
    pub fn get_grid_view(
        &self,
        width: Option<u16>,
        height: Option<u16>,
        _at_time: Option<chrono::DateTime<Utc>>,
        _from_line: Option<u64>,
    ) -> Result<GridViewResponse, String> {
        let guard = self.terminal_tracker.lock().unwrap();
        
        if let Some(ref tracker_arc) = *guard {
            let tracker = tracker_arc.lock().unwrap();
            let history = tracker.get_history();
            let view = GridView::new(history);
            
            // Get the grid with optional dimension override
            let dimensions = width.zip(height).map(|(w, h)| (w, h));
            let grid = view.derive_realtime(dimensions)
                .map_err(|e| format!("Failed to get grid view: {:?}", e))?;
            
            // Convert grid to text rows
            let mut rows = Vec::new();
            let mut ansi_rows = Vec::new();
            
            for row in 0..grid.height {
                let mut line = String::new();
                let mut ansi_line = String::new();
                
                for col in 0..grid.width {
                    if let Some(cell) = grid.get_cell(row, col) {
                        // Skip null characters (wide character continuations)
                        if cell.char != '\0' {
                            line.push(cell.char);
                            
                            // Build ANSI version with colors
                            let ansi_cell = format_cell_ansi(&cell);
                            ansi_line.push_str(&ansi_cell);
                        }
                    }
                }
                
                // Trim trailing spaces for cleaner output
                let line = line.trim_end().to_string();
                let ansi_line = ansi_line.trim_end().to_string();
                
                rows.push(line);
                ansi_rows.push(ansi_line);
            }
            
            Ok(GridViewResponse {
                width: grid.width,
                height: grid.height,
                cursor_row: grid.cursor.row,
                cursor_col: grid.cursor.col,
                cursor_visible: grid.cursor.visible,
                rows,
                ansi_rows: Some(ansi_rows),
                timestamp: Utc::now(),
                start_line: 0, // TODO: Calculate from line counters
                end_line: grid.height as u64 - 1,
            })
        } else {
            Err("Terminal tracker not initialized".to_string())
        }
    }
    
    /// Get terminal statistics
    pub fn get_stats(&self) -> Result<StatsResponse, String> {
        let guard = self.terminal_tracker.lock().unwrap();
        
        if let Some(ref tracker_arc) = *guard {
            let tracker = tracker_arc.lock().unwrap();
            let history = tracker.get_history();
            let history_lock = history.lock().unwrap();
            
            // Calculate memory usage and stats
            let stats = history_lock.get_stats();
            
            Ok(StatsResponse {
                history_size_bytes: stats.memory_usage,
                total_deltas: stats.total_deltas,
                total_snapshots: stats.total_snapshots,
                current_dimensions: (80, 24), // TODO: Get from tracker
                session_duration_secs: stats.session_duration.as_secs(),
            })
        } else {
            Err("Terminal tracker not initialized".to_string())
        }
    }
    
    /// Clear terminal history
    pub fn clear_history(&self) -> Result<(), String> {
        let guard = self.terminal_tracker.lock().unwrap();
        
        if let Some(ref tracker_arc) = *guard {
            let tracker = tracker_arc.lock().unwrap();
            let history = tracker.get_history();
            let mut history_lock = history.lock().unwrap();
            history_lock.clear();
            Ok(())
        } else {
            Err("Terminal tracker not initialized".to_string())
        }
    }
}

/// Response structure for grid view
pub struct GridViewResponse {
    pub width: u16,
    pub height: u16,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub cursor_visible: bool,
    pub rows: Vec<String>,
    pub ansi_rows: Option<Vec<String>>,
    pub timestamp: chrono::DateTime<Utc>,
    pub start_line: u64,
    pub end_line: u64,
}

/// Response structure for statistics
pub struct StatsResponse {
    pub history_size_bytes: usize,
    pub total_deltas: u64,
    pub total_snapshots: usize,
    pub current_dimensions: (u16, u16),
    pub session_duration_secs: u64,
}

/// Format a cell with ANSI escape codes
fn format_cell_ansi(cell: &crate::server::terminal_state::Cell) -> String {
    use crate::server::terminal_state::Color;
    
    let mut codes = Vec::new();
    
    // Handle foreground color
    match &cell.fg_color {
        Color::Default => {},
        Color::Indexed(idx) if *idx < 8 => {
            codes.push(format!("3{}", idx));
        },
        Color::Indexed(idx) if *idx < 16 => {
            codes.push(format!("9{}", idx - 8));
        },
        Color::Indexed(idx) => {
            codes.push(format!("38;5;{}", idx));
        },
        Color::Rgb(r, g, b) => {
            codes.push(format!("38;2;{};{};{}", r, g, b));
        },
    }
    
    // Handle background color
    match &cell.bg_color {
        Color::Default => {},
        Color::Indexed(idx) if *idx < 8 => {
            codes.push(format!("4{}", idx));
        },
        Color::Indexed(idx) if *idx < 16 => {
            codes.push(format!("10{}", idx - 8));
        },
        Color::Indexed(idx) => {
            codes.push(format!("48;5;{}", idx));
        },
        Color::Rgb(r, g, b) => {
            codes.push(format!("48;2;{};{};{}", r, g, b));
        },
    }
    
    // Handle attributes
    if cell.attributes.bold { codes.push("1".to_string()); }
    if cell.attributes.italic { codes.push("3".to_string()); }
    if cell.attributes.underline { codes.push("4".to_string()); }
    if cell.attributes.reverse { codes.push("7".to_string()); }
    
    if codes.is_empty() {
        cell.char.to_string()
    } else {
        format!("\x1b[{}m{}\x1b[0m", codes.join(";"), cell.char)
    }
}
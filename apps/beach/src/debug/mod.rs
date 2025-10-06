pub mod ipc;
pub mod server;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DiagnosticRequest {
    GetCursorState,
    GetTerminalDimensions,
    GetCacheState,
    GetRendererState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorState {
    pub row: usize,
    pub col: usize,
    pub seq: u64,
    pub visible: bool,
    pub authoritative: bool,
    pub cursor_support: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalDimensions {
    pub rows: usize,
    pub cols: usize,
    pub viewport_rows: usize,
    pub viewport_cols: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheState {
    pub grid_rows: usize,
    pub grid_cols: usize,
    pub row_offset: u64,
    pub first_row_id: Option<u64>,
    pub last_row_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RendererState {
    pub cursor_row: u64,
    pub cursor_col: usize,
    pub cursor_visible: bool,
    pub base_row: u64,
    pub viewport_top: u64,
    pub cursor_viewport_position: Option<(u16, u16)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DiagnosticResponse {
    CursorState(CursorState),
    TerminalDimensions(TerminalDimensions),
    CacheState(CacheState),
    RendererState(RendererState),
    Error(String),
}

use crate::debug::ipc::send_diagnostic_request;
use crate::debug::{DiagnosticRequest, DiagnosticResponse};
use crate::terminal::cli::DebugArgs;
use crate::terminal::error::CliError;

pub fn run(args: DebugArgs) -> Result<(), CliError> {
    let session_id = &args.session_id;

    let requests = if let Some(query) = args.query {
        match query.to_lowercase().as_str() {
            "cursor" => vec![
                DiagnosticRequest::GetCursorState,
                DiagnosticRequest::GetRendererState,
            ],
            "dimensions" => vec![DiagnosticRequest::GetTerminalDimensions],
            "cache" => vec![DiagnosticRequest::GetCacheState],
            "renderer" => vec![DiagnosticRequest::GetRendererState],
            "all" => vec![
                DiagnosticRequest::GetCursorState,
                DiagnosticRequest::GetTerminalDimensions,
                DiagnosticRequest::GetCacheState,
                DiagnosticRequest::GetRendererState,
            ],
            _ => {
                return Err(CliError::InvalidArgument(format!(
                    "Unknown query type: {}. Valid options: cursor, dimensions, cache, renderer, all",
                    query
                )));
            }
        }
    } else {
        vec![
            DiagnosticRequest::GetCursorState,
            DiagnosticRequest::GetRendererState,
            DiagnosticRequest::GetTerminalDimensions,
            DiagnosticRequest::GetCacheState,
        ]
    };

    for request in requests {
        let response = send_diagnostic_request(session_id, request)
            .map_err(|err| CliError::Runtime(err.to_string()))?;

        print_response(&response);
    }

    Ok(())
}

fn print_response(response: &DiagnosticResponse) {
    match response {
        DiagnosticResponse::CursorState(state) => {
            println!("=== Cursor State (Client Cache) ===");
            println!("  Position:      row={}, col={}", state.row, state.col);
            println!("  Sequence:      {}", state.seq);
            println!("  Visible:       {}", state.visible);
            println!("  Authoritative: {}", state.authoritative);
            println!(
                "  Cursor support: {} (server {}sending cursor frames)",
                state.cursor_support,
                if state.cursor_support { "IS " } else { "NOT " }
            );
            println!();
        }
        DiagnosticResponse::RendererState(state) => {
            println!("=== Renderer State (What's Actually Rendered) ===");
            println!(
                "  Cursor:        row={}, col={}",
                state.cursor_row, state.cursor_col
            );
            println!("  Cursor visible: {}", state.cursor_visible);
            println!("  Base row:      {}", state.base_row);
            println!("  Viewport top:  {}", state.viewport_top);
            if let Some((col, row)) = state.cursor_viewport_position {
                println!("  Cursor viewport pos: ({}, {}) - VISIBLE IN TUI", col, row);
            } else {
                println!("  Cursor viewport pos: None - NOT VISIBLE IN TUI");
            }
            println!();
        }
        DiagnosticResponse::TerminalDimensions(dims) => {
            println!("=== Terminal Dimensions ===");
            println!("  Grid:     {}x{} (rows x cols)", dims.rows, dims.cols);
            println!(
                "  Viewport: {}x{} (rows x cols)",
                dims.viewport_rows, dims.viewport_cols
            );
            println!();
        }
        DiagnosticResponse::CacheState(cache) => {
            println!("=== Cache State ===");
            println!(
                "  Grid size:   {}x{} (rows x cols)",
                cache.grid_rows, cache.grid_cols
            );
            println!("  Row offset:  {}", cache.row_offset);
            if let Some(first) = cache.first_row_id {
                println!("  First row:   {}", first);
            }
            if let Some(last) = cache.last_row_id {
                println!("  Last row:    {}", last);
            }
            println!();
        }
        DiagnosticResponse::Error(err) => {
            eprintln!("Error: {}", err);
        }
    }
}

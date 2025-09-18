# Client Unified Grid Implementation Plan

## Overview
This document outlines the implementation plan to transform the beach client from a viewport-based subscription model to a unified grid model where the client maintains a complete local cache of all terminal history.

## Current Issues
- Client uses viewport-based subscriptions, sending `ViewportChanged` messages when scrolling
- Grid is dynamically sized based on received data, not server history size
- Placeholder characters (⏳) persist incorrectly when merging snapshots
- Complex historical/realtime mode switching causes rendering issues

## Target Architecture

### Key Principles
1. **Unified Grid**: Client maintains a single grid sized to match the full server terminal history
2. **No ViewportChanged**: Client never requests specific line ranges; server pushes all data
3. **Progressive Loading**: Server streams history to client until complete
4. **Local Scrolling**: All scrolling is handled client-side using the local cache
5. **Placeholder Management**: Placeholders (⏳) only appear in first cell of unloaded rows

## Implementation Plan

### Phase 1: Protocol Changes

#### 1.1 Enhanced Initial Subscription
**File**: `apps/beach/src/protocol/client_messages.rs`
- Modify `Subscribe` message to include:
  ```rust
  pub struct Subscribe {
      pub subscription_id: String,
      pub dimensions: Dimensions,
      pub initial_fetch_size: u32,  // NEW: How many rows to send initially (e.g., 500)
      pub stream_history: bool,      // NEW: Whether to stream full history
  }
  ```

#### 1.2 New Server Messages
**File**: `apps/beach/src/protocol/server_messages.rs`
- Add new message types:
  ```rust
  pub enum ServerMessage {
      // ... existing messages ...

      // NEW: Sent with initial subscription response
      HistoryMetadata {
          subscription_id: String,
          total_lines: u64,        // Total lines in server history
          oldest_line: u64,        // Oldest line number
          latest_line: u64,        // Most recent line number
          terminal_width: u16,     // Server terminal width
          terminal_height: u16,    // Server visible height
      },

      // NEW: Streamed history chunks
      HistoryChunk {
          subscription_id: String,
          start_line: u64,
          end_line: u64,
          rows: Vec<Vec<Cell>>,    // Row data
          is_final: bool,          // True if this is the last chunk
      },
  }
  ```

### Phase 2: Server-Side Changes

#### 2.1 Subscription Handler Updates
**File**: `apps/beach/src/subscription/hub.rs`

**Changes to `handle_client_message`**:
1. When handling `Subscribe`:
   - Send `HistoryMetadata` immediately with full history size
   - Send initial `Snapshot` with the most recent `initial_fetch_size` rows
   - Start background task to stream remaining history

2. Remove `ViewportChanged` handling entirely

**New method `stream_history_to_client`**:
```rust
async fn stream_history_to_client(&self, subscription_id: String) {
    // Algorithm:
    // 1. Get subscription and check what lines client already has
    // 2. Calculate remaining lines to send
    // 3. Send in chunks of 100-200 rows, oldest to newest
    // 4. Rate limit to avoid overwhelming client
    // 5. Mark each chunk with is_final=true for the last one
}
```

#### 2.2 Terminal State Data Source
**File**: `apps/beach/src/server/terminal_state/data_source_impl.rs`

**New method `get_history_range`**:
```rust
pub async fn get_history_range(&self, start_line: u64, count: u16) -> Result<Vec<Vec<Cell>>> {
    // Retrieve specific line range from history
    // Return actual cell data, not a Grid
}
```

### Phase 3: Client-Side Changes

#### 3.1 Grid Renderer Overhaul
**File**: `apps/beach/src/client/grid_renderer.rs`

**Major changes**:

1. **Grid initialization based on server metadata**:
```rust
pub struct GridRenderer {
    // Full-size grid matching server history
    grid: Vec<Vec<Option<Cell>>>,  // Option to track loaded vs unloaded

    // Server dimensions
    server_total_lines: u64,
    server_width: u16,
    server_visible_height: u16,

    // Line mapping
    line_offset: u64,  // Maps grid index to absolute line number

    // Scroll state (pure client-side)
    scroll_position: u64,  // Current top line being displayed

    // No more historical mode, view_line, or complex viewport tracking
}

impl GridRenderer {
    pub fn new_with_metadata(metadata: HistoryMetadata) -> Self {
        // Initialize grid with full history size
        let total_rows = metadata.total_lines as usize;
        let mut grid = Vec::with_capacity(total_rows);

        // Initialize all rows as None (not yet loaded)
        for _ in 0..total_rows {
            grid.push(vec![None; metadata.terminal_width as usize]);
        }

        Self {
            grid,
            server_total_lines: metadata.total_lines,
            server_width: metadata.terminal_width,
            server_visible_height: metadata.terminal_height,
            line_offset: metadata.oldest_line,
            scroll_position: metadata.latest_line.saturating_sub(metadata.terminal_height as u64),
        }
    }

    pub fn apply_initial_snapshot(&mut self, snapshot: Grid) {
        // Map snapshot lines to grid positions
        let start_idx = (snapshot.start_line - self.line_offset) as usize;

        for (i, row) in snapshot.cells.iter().enumerate() {
            let grid_idx = start_idx + i;
            if grid_idx < self.grid.len() {
                self.grid[grid_idx] = row.iter().map(|c| Some(c.clone())).collect();
            }
        }
    }

    pub fn apply_history_chunk(&mut self, chunk: HistoryChunk) {
        // Map chunk lines to grid positions
        let start_idx = (chunk.start_line - self.line_offset) as usize;

        for (i, row) in chunk.rows.iter().enumerate() {
            let grid_idx = start_idx + i;
            if grid_idx < self.grid.len() {
                self.grid[grid_idx] = row.iter().map(|c| Some(c.clone())).collect();
            }
        }
    }

    pub fn apply_delta(&mut self, delta: &GridDelta) {
        // Apply delta to the appropriate row
        // Deltas always apply to recent rows (near the end of grid)
        for change in &delta.cell_changes {
            let row_idx = (change.row as u64 + self.server_total_lines - self.server_visible_height as u64) as usize;
            if row_idx < self.grid.len() {
                // Ensure row exists and has data
                if self.grid[row_idx].is_empty() {
                    self.grid[row_idx] = vec![None; self.server_width as usize];
                }
                if change.col < self.server_width {
                    self.grid[row_idx][change.col as usize] = Some(change.new_cell.clone());
                }
            }
        }

        // Handle new lines being added (scrolling)
        if let Some(dim_change) = &delta.dimension_change {
            if dim_change.lines_added > 0 {
                // Add new rows at the end
                for _ in 0..dim_change.lines_added {
                    self.grid.push(vec![None; self.server_width as usize]);
                    self.server_total_lines += 1;
                }
            }
        }
    }

    pub fn scroll(&mut self, delta: i64) {
        // Simple client-side scrolling
        let new_position = (self.scroll_position as i64 + delta)
            .max(0)
            .min((self.server_total_lines - self.server_visible_height as u64) as i64) as u64;
        self.scroll_position = new_position;
    }

    pub fn render(&self, frame: &mut Frame) {
        let visible_height = frame.area().height as usize;
        let start_idx = self.scroll_position as usize;
        let end_idx = (start_idx + visible_height).min(self.grid.len());

        let mut lines = Vec::new();

        for row_idx in start_idx..end_idx {
            if row_idx < self.grid.len() {
                let row = &self.grid[row_idx];

                // Check if row has any data
                let has_data = row.iter().any(|cell| cell.is_some());

                if !has_data {
                    // Show placeholder in first cell only
                    let mut spans = vec![Span::raw("⏳")];
                    // Fill rest with spaces
                    for _ in 1..self.server_width {
                        spans.push(Span::raw(" "));
                    }
                    lines.push(Line::from(spans));
                } else {
                    // Render actual content
                    let mut spans = Vec::new();
                    for cell_opt in row {
                        match cell_opt {
                            Some(cell) => {
                                spans.push(Span::styled(cell.char.to_string(), cell_to_style(cell)));
                            }
                            None => {
                                // Individual missing cell (shouldn't happen after initial load)
                                spans.push(Span::raw(" "));
                            }
                        }
                    }
                    lines.push(Line::from(spans));
                }
            }
        }

        let paragraph = Paragraph::new(lines);
        frame.render_widget(paragraph, frame.area());
    }
}
```

#### 3.2 Terminal Client Updates
**File**: `apps/beach/src/client/terminal_client.rs`

**Changes to message handling**:

1. **Remove all ViewportChanged logic**
2. **Handle new message types**:
```rust
match message {
    ServerMessage::HistoryMetadata { .. } => {
        // Initialize grid with full size
        let mut renderer = self.grid_renderer.lock().await;
        *renderer = GridRenderer::new_with_metadata(metadata);
    }

    ServerMessage::Snapshot { .. } => {
        // Apply initial snapshot
        self.grid_renderer.lock().await.apply_initial_snapshot(grid);
    }

    ServerMessage::HistoryChunk { .. } => {
        // Apply history chunk to grid
        self.grid_renderer.lock().await.apply_history_chunk(chunk);

        if is_final {
            // Log that all history has been loaded
            eprintln!("Full terminal history loaded");
        }
    }

    ServerMessage::Delta { .. } => {
        // Apply delta as before, but to the unified grid
        self.grid_renderer.lock().await.apply_delta(&delta);
    }
}
```

3. **Simplify scroll handling**:
```rust
// In handle_key_event
KeyCode::Up | KeyCode::PageUp => {
    let delta = if key.code == KeyCode::PageUp { -10 } else { -1 };
    self.grid_renderer.lock().await.scroll(delta);
}
KeyCode::Down | KeyCode::PageDown => {
    let delta = if key.code == KeyCode::PageDown { 10 } else { 1 };
    self.grid_renderer.lock().await.scroll(delta);
}
```

### Phase 4: Optimization

#### 4.1 Memory Management
- Implement row eviction for very old history if memory becomes an issue
- Use compression for cell data in the grid
- Consider using a sparse data structure for mostly-empty rows

#### 4.2 Streaming Optimization
- Prioritize sending rows near the current view position
- Batch small deltas together
- Use binary encoding for history chunks

### Phase 5: Testing

#### 5.1 Unit Tests
- Test grid initialization with various history sizes
- Test delta application at grid boundaries
- Test scrolling limits
- Test placeholder rendering

#### 5.2 Integration Tests
- Test full history streaming from server to client
- Test delta application during history streaming
- Test scroll performance with large histories
- Test memory usage with maximum history

## Migration Path

1. **Backward Compatibility**: Keep ViewportChanged handling initially but mark as deprecated
2. **Feature Flag**: Add a feature flag to enable unified grid mode
3. **Gradual Rollout**: Test with small histories first, then scale up
4. **Performance Monitoring**: Track memory usage and scroll performance

## Success Criteria

1. ✅ Client grid is sized to match full server history from the start
2. ✅ No ViewportChanged messages sent from client
3. ✅ Scrolling is instant (pure client-side)
4. ✅ Placeholders (⏳) only appear in first cell of unloaded rows
5. ✅ All rows eventually load in background
6. ✅ Deltas correctly update the unified grid
7. ✅ Memory usage is reasonable even with large histories

## Timeline

- **Week 1**: Protocol changes and server-side streaming
- **Week 2**: Client grid renderer overhaul
- **Week 3**: Integration and testing
- **Week 4**: Optimization and performance tuning
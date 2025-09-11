# Overscan Blank Lines Issue - Root Cause Analysis

## Problem Statement

When running the beach client with overscan mode (requesting 2x terminal height for smooth scrolling), extra blank lines appear on the client display. The client shows approximately 62 blank rows after 2 rows of actual content, making the terminal appear mostly empty.

## Log Locations

- **Client Debug Log**: `/tmp/client.log`
- **Server Debug Log**: `/tmp/server.log`  
- **Client Debug Recorder (JSONL)**: `/tmp/client-debug.jsonl`
- **Server Debug Recorder (JSONL)**: `/tmp/server-debug.jsonl` (note: corrupted/not proper JSONL)
- **Render Debug Log**: `/tmp/render-debug.log`
- **Analysis Scripts**: `/Users/arellidow/Documents/workspace/beach/scripts/analyze-debug-recording.py`

## Evidence from Latest Debug Logs

### 1. Client Grid State Analysis

From deep analysis of `/tmp/client-debug.jsonl`:

```
=== Grid #1-5 (consistent pattern) ===
Dimensions: 100x64
Content rows: [0, 1]
Content range: rows 0 to 1
Blank groups: [(2, 63)]
  - Blank rows 2-63 (62 rows)

Sample content:
  Row 0: 'Restored session: Wed Sep 10 11:24:58 EDT 2025'
  Row 1: '(base) arellidow@Arels-MacBook-Pro ~ % echo'
  Row 2-63: [ALL BLANK]

Pattern Analysis:
- Content at top: TRUE
- Content at bottom: FALSE
- Has interspersed blanks: FALSE
```

**Key Finding**: The grids consistently have content at the TOP (rows 0-1) with 62 blank rows below.

### 2. Dimension Discrepancies

```
Client subscribes with: 109x128 (2x overscan)
Grid states received: 100x64
Server truncation: Yes (from 64 to 64 - no actual truncation happening)
```

The server is NOT providing overscan height - it's sending regular terminal height.

### 3. Render Debug Analysis

From `/tmp/render-debug.log` after our fixes:
```
area_height: 63
grid.cells.len(): 64
local_height: 64
scroll_offset: 0
visible_height: 63
start_row: 1    <- Bottom anchoring working (64-63=1)
end_row: 64
```

The client-side bottom anchoring IS working correctly, but it doesn't help because the content is at the TOP of the grid.

## Root Cause Identified

The issue is in the **Alacritty backend's grid synchronization** at `/Users/arellidow/Documents/workspace/beach/apps/beach/src/server/terminal_state/alacritty_backend.rs`:

```rust
// Line 215-227 in sync_grid()
for line in 0..alac_lines.min(self.height as usize) {
    for col in 0..alac_cols.min(self.width as usize) {
        let point = alacritty_terminal::index::Point {
            line: alacritty_terminal::index::Line(line as i32),
            column: alacritty_terminal::index::Column(col),
        };
        let cell_ref = &term.grid()[point];
        let cell = self.convert_cell(cell_ref);
        self.current_grid.set_cell(line as u16, col as u16, cell);
    }
}
```

**The Problem**: This code copies Alacritty's grid starting from line 0. When the terminal has minimal content (just 2 lines), Alacritty places it at the top of its screen buffer, and we copy it directly to rows 0-1 of our grid, leaving rows 2-63 empty.

## Attempted Fixes

### Fix 1: Per-Subscription Delta Generation ✓ (Correct but not the issue)
**File**: `/Users/arellidow/Documents/workspace/beach/apps/beach/src/subscription/hub.rs`
- Added `previous_grid` tracking per subscription
- Generate subscription-specific deltas
- **Result**: Good practice but didn't fix the blank lines

### Fix 2: Client-Side Bottom Anchoring ✓ (Partially helps)
**File**: `/Users/arellidow/Documents/workspace/beach/apps/beach/src/client/grid_renderer.rs`
- Changed from top anchoring to bottom anchoring
- **Result**: Would work if content was at bottom of grid, but content is at top

### Fix 3: Server-Side Bottom Truncation ✗ (Doesn't apply)
**File**: `/Users/arellidow/Documents/workspace/beach/apps/beach/src/server/terminal_state/grid_view.rs`
- Changed `truncate_to_height` to keep bottom rows instead of top
- **Result**: No effect because grids aren't being truncated (64 height requested, 64 provided)

### Fix 4: Debug Recorder Error Logging ✓ (Diagnostic)
**File**: `/Users/arellidow/Documents/workspace/beach/apps/beach/src/client/terminal_client.rs`
- Added error logging for serialization failures
- **Result**: Helps identify issues but doesn't fix blank lines

## The Real Solution Needed

The fix needs to be in the Alacritty backend's `sync_grid()` method. Options:

### Option 1: Bottom-Align Content in Grid
Modify the sync_grid() to place content at the BOTTOM of the grid:

```rust
// Calculate where content should start in our grid
let content_height = /* actual non-empty lines from Alacritty */;
let start_row = if content_height < self.height {
    self.height - content_height  // Place at bottom
} else {
    0  // Content fills entire grid
};

// Copy with offset
for line in 0..alac_lines.min(self.height as usize) {
    let target_row = start_row + line as u16;
    // ... copy cells to target_row instead of line
}
```

### Option 2: Track Actual Terminal Bottom
Alacritty tracks the "active" area of the terminal. We should:
1. Find where the actual terminal content ends
2. Align our grid based on that
3. Use Alacritty's viewport and display offset concepts

### Option 3: Send Only Non-Empty Content
Instead of always sending a full 64-row grid:
1. Detect actual content bounds
2. Send a smaller grid with just the content
3. Let the client handle positioning

## Why Overscan Isn't Working

The client requests 109x128 (2x height) for overscan, but:
1. Server ignores the height request and sends 64-row grids
2. The `derive_realtime()` function caps at actual terminal height
3. No scrollback buffer content is included
4. Overscan is effectively non-functional

## Testing Commands

```bash
# Clear render debug log
rm /tmp/render-debug.log

# Run server
cargo run -p beach -- --debug-recorder /tmp/server-debug.jsonl --debug-log /tmp/server.log

# Run client with overscan
cargo run -p beach -- --debug-recorder /tmp/client-debug.jsonl --debug-log /tmp/client.log -v --join <session-url>

# Analyze
python3 scripts/analyze-debug-recording.py /tmp/client-debug.jsonl
tail -20 /tmp/render-debug.log
```

## Next Steps

1. **Immediate Fix**: Modify `AlacrittyBackend::sync_grid()` to bottom-align content
2. **Proper Overscan**: Implement actual scrollback buffer access for overscan
3. **Grid Optimization**: Only send content that exists, not full empty grids
4. **Protocol Enhancement**: Add explicit viewport/content bounds in the protocol

## Conclusion

The blank lines issue is caused by the Alacritty backend placing terminal content at the TOP of the grid (rows 0-1) with empty rows below (2-63), combined with the server not implementing actual overscan (sending 64 rows instead of requested 128). The client-side rendering fixes don't help because they're correctly rendering what they receive - a grid with content at the top and 62 blank rows.
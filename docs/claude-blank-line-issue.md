# Blank Lines Issue Analysis

## Problem Description

When running TUI applications (or any command with multi-line output) through beach, the client displays extra blank lines that are not present in the server's terminal. This creates a visual discrepancy where content appears to have additional spacing on the client side.

### Example

**Server Terminal Output:**
```
> hiya tell me a joke

⏺ notifications - task_status (MCP)(message: "Started processing")
  ⎿  Error: Output validation error: True is not of type 'string'

⏺ notifications - task_status (MCP)(message: "Telling a joke")
  ⎿  Error: Output validation error: True is not of type 'string'

⏺ Why don't programmers like nature?

  It has too many bugs! 🐛
```

**Client Terminal Output:**
```
> hiya tell me a joke                                                                               
                                                                                                    
                                                                                                    
⏺ notifications - task_status (MCP)(message: "Started processing")                                  
  ⎿  Error: Output validation error: True is not of type 'string'                                   
                                                                                                    
                                                                                                    
⏺ notifications - task_status (MCP)(message: "Telling a joke")                                      
  ⎿  Error: Output validation error: True is not of type 'string'                                   
                                                                                                    
                                                                                                    
⏺ Why don't programmers like nature?                                                                
                                                                                                    
                                                                                                    
  It has too many bugs! 🐛
```

## Investigation Process

### 1. Debug Recorder Implementation

To diagnose this issue, we implemented a debug recorder system that captures all subscription events:

**Files Created:**
- `/Users/arellidow/Documents/workspace/beach/apps/beach/src/debug_recorder.rs` - Core recording functionality
- `/Users/arellidow/Documents/workspace/beach/scripts/analyze-debug-recording.py` - Analysis tool

**CLI Usage:**
```bash
# Server with debug recording
cargo run -p beach -- --debug-recorder /tmp/server-debug.jsonl --debug-log /tmp/server.log -- bash

# Client with debug recording
cargo run -p beach -- --join <session> --debug-recorder /tmp/client-debug.jsonl --debug-log /tmp/client.log
```

### 2. Log Analysis

**Log Files Examined:**
- `/tmp/client-debug.jsonl` - Client subscription events (10.7MB for a short session)
- `/tmp/server-debug.jsonl` - Server events (would capture server-side events once implemented)
- `/tmp/client.log` - Client debug log
- `/tmp/server.log` - Server debug log

### 3. Key Findings from Analysis

#### 3.1 Client Grid Dimensions
```python
# From analyze-debug-recording.py output:
CLIENT_GRID: 100x64 (realtime)  # Client requests overscan (2x height)
```

The client requests a 100x64 grid (overscan) when the actual terminal is 100x32. This is done for smooth scrolling support.

#### 3.2 Delta Message Analysis
```
📦 Message Sequence Analysis
Snapshots received: 0
Deltas received: 250+
```

The client receives only delta updates after the initial snapshot, no full grid refreshes.

#### 3.3 Blank Lines Detection
```
Found 16000 events with blank lines:
  - Row 0 in 100x64 grid
  - Row 1 in 100x64 grid
  - Row 2 in 100x64 grid
  ...
```

Many rows in the client's grid remain blank or contain stale content.

#### 3.4 Missing Line Shift Operations
```bash
# Searching for line shift operations in recordings
$ tail -1000 /tmp/client-debug.jsonl | grep -o '"line_shift"' | wc -l
0
```

No line shift operations are included in the GridDelta protocol, which means scrolling operations aren't explicitly communicated.

## Root Cause Analysis

### The Architecture

1. **PTY → Terminal Emulator**: Both the server (using Alacritty backend) and the actual terminal receive the same byte sequences from the PTY
2. **Server Terminal State**: Maintains a 100x32 grid matching the actual terminal dimensions
3. **Client Subscription**: Requests a 100x64 grid for overscan/smooth scrolling
4. **Delta Generation**: Server generates deltas based on changes to its 32-row terminal

### The Bug

The bug is located in `/Users/arellidow/Documents/workspace/beach/apps/beach/src/subscription/hub.rs`, specifically in the `push_delta` method (lines 475-506):

```rust
pub async fn push_delta(&self, delta: crate::server::terminal_state::GridDelta) -> Result<()> {
    // ...
    for subscription in subscriptions.values_mut() {
        subscription.current_sequence = subscription.current_sequence.saturating_add(1);
        let msg = ServerMessage::Delta {
            subscription_id: subscription.id.clone(),
            sequence: subscription.current_sequence,
            changes: delta.clone(),  // <-- BUG: Same delta for all subscriptions!
            timestamp: chrono::Utc::now().timestamp(),
        };
        // ...
    }
}
```

**The Problem:** The subscription hub broadcasts the SAME delta to all subscriptions, regardless of their different dimensions.

### What Happens During Scrolling

1. **Server Terminal Updates:**
   - New line appears at bottom (row 31 in 32-row terminal)
   - Content scrolls up: row 1 → row 0, row 2 → row 1, etc.
   - Old row 0 content moves to scrollback buffer

2. **Delta Generated:**
   - Contains cell changes for rows 0-31 (server's view)
   - Represents the new state after scrolling

3. **Client Receives Delta:**
   - Has a 64-row grid expecting updates
   - Applies changes only to rows 0-31
   - Rows 32-63 remain unchanged (blank or stale)

4. **Result:**
   - Client's rows 0-31 get updated with server's content
   - Client's rows 32-63 stay blank
   - When rendering, these blank rows create visual gaps

## Proposed Fix

### Solution 1: Per-Subscription Delta Generation (Recommended)

**Location to modify:** `/Users/arellidow/Documents/workspace/beach/apps/beach/src/subscription/hub.rs`

Instead of broadcasting the same delta:
1. Maintain separate GridViews for each subscription with their requested dimensions
2. Track previous state per subscription
3. Generate subscription-specific deltas based on each view's changes

```rust
// Conceptual implementation
pub struct Subscription {
    // ... existing fields ...
    previous_grid: Grid,  // Track last sent state
    view_dimensions: Dimensions,  // Track requested dimensions
}

pub async fn push_terminal_update(&self) -> Result<()> {
    let subscriptions = self.subscriptions.write().await;
    
    for subscription in subscriptions.values_mut() {
        // Get current view for this subscription's dimensions
        let current_view = self.terminal_source
            .snapshot(subscription.view_dimensions)
            .await?;
        
        // Generate delta specific to this subscription
        let delta = compute_delta(&subscription.previous_grid, &current_view);
        
        // Send subscription-specific delta
        let msg = ServerMessage::Delta {
            subscription_id: subscription.id.clone(),
            sequence: subscription.current_sequence,
            changes: delta,
            timestamp: chrono::Utc::now().timestamp(),
        };
        
        // Update tracked state
        subscription.previous_grid = current_view;
        
        self.send_to_subscription(subscription, msg).await?;
    }
}
```

### Solution 2: Add Line Shift Operations to Protocol

**Location to modify:** `/Users/arellidow/Documents/workspace/beach/apps/beach/src/server/terminal_state/grid_delta.rs`

Extend GridDelta to explicitly communicate scrolling:
```rust
pub struct GridDelta {
    // ... existing fields ...
    
    /// Line shift operations for scrolling
    pub line_shifts: Option<LineShift>,
}

pub struct LineShift {
    pub direction: ShiftDirection,  // Up or Down
    pub count: u16,                 // Number of lines shifted
    pub range: Option<(u16, u16)>,  // Affected line range
    pub fill_lines: Vec<u16>,       // New blank lines created by shift
}
```

### Solution 3: Disable Overscan (Temporary Workaround)

**Location to modify:** `/Users/arellidow/Documents/workspace/beach/apps/beach/src/client/terminal_client.rs`

Change line 572:
```rust
// Current (with overscan):
let overscan_height = height * 2;

// Temporary fix (no overscan):
let overscan_height = height;
```

This would make client request exact terminal dimensions, avoiding the mismatch.

## Impact

This bug affects:
- All TUI applications (vim, nano, htop, etc.)
- Multi-line command output
- Any scenario where terminal content scrolls

The issue is particularly noticeable when:
- Running commands that produce multiple lines of output quickly
- Using applications that update the full screen
- Scrolling through content in the terminal

## Next Steps

1. **Immediate:** Test the temporary workaround (disable overscan) to confirm the hypothesis
2. **Short-term:** Implement per-subscription delta generation (Solution 1)
3. **Long-term:** Consider adding explicit scroll operations to the protocol for efficiency

## Files Modified During Investigation

- `apps/beach/src/debug_recorder.rs` - New debug recording system
- `apps/beach/src/client/terminal_client.rs` - Added debug recording hooks
- `apps/beach/src/lib.rs` - Added debug_recorder module
- `apps/beach/src/main.rs` - Added debug_recorder module and CLI parameter passing
- `scripts/analyze-debug-recording.py` - Analysis tool for debug recordings

## Conclusion

The blank lines issue is caused by a fundamental mismatch in how the subscription hub handles different client view dimensions. The hub broadcasts the same delta (based on the server's 32-row view) to all clients, including those with 64-row overscan views. This causes the lower half of the client's grid to remain blank or contain stale content, creating the visual appearance of extra blank lines.

The fix requires making the subscription hub aware of per-subscription dimensions and generating appropriate deltas for each subscription's specific view.
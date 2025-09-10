# Terminal Client Input/Output Issues

## Problem Summary

The beach terminal client is experiencing two critical issues that prevent proper terminal emulation:

1. **Blocking I/O Issue**: The client's event loop blocks on stdin input handling, preventing server messages from being processed
2. **Predictive Echo Issue**: Underlined predictive text is not being replaced by server confirmations

These issues manifest as garbled terminal output where user input is incorrectly displayed. For example, when typing "echo world", the client shows "echo hi" with fragmented output like "ch d" appearing on the server side.

## Detailed Problem Analysis

### Issue 1: Blocking Event Loop

**Current Behavior:**
- The client's main event loop in `terminal_client.rs` uses a synchronous blocking call to `event::read()`
- While waiting for keyboard input, the loop cannot process incoming server messages
- This creates a race condition where server deltas queue up and are processed in bursts

**Code Location:** `apps/beach/src/client/terminal_client.rs:303-326`

```rust
// Current problematic implementation
loop {
    // This blocks the entire thread waiting for keyboard input
    if event::poll(Duration::from_millis(100))? {
        match event::read()? {
            Event::Key(key_event) => {
                self.handle_key_event(key_event).await?;
            }
            // ...
        }
    }
    // Server messages only processed when not blocked on input
    self.handle_server_messages().await?;
}
```

### Issue 2: Predictive Echo Not Replaced

**Current Behavior:**
- When a user types, the client shows underlined "predictive" text immediately
- This predictive text should be replaced when the server echo arrives
- Currently, the predictive text remains visible alongside the server echo
- Results in duplicate/garbled text display

**Root Cause:**
- The terminal is not actually entering raw mode
- The shell (bash) is still processing input directly
- Both the shell and beach are trying to handle the same keystrokes
- Predictive echo replacement logic is never triggered because the terminal state machine isn't properly initialized

## Evidence from Debug Logs

From `/tmp/test-client2.log`:
```
[21:45:51.267] Client sending Subscribe message
[21:45:51.273] Client received SubscriptionAck
[21:45:51.273] Client received initial Snapshot
[21:45:51.303] [WebRTC] Transport recv (legacy): 830 bytes
```

The client successfully subscribes and receives data, but the terminal UI never properly initializes.

From user testing:
- User typed: "echo world"
- Client displayed: "echo hi\nhi\n(base) arellidow@Arels-MacBook-Pro ~ % ch d"
- This shows the shell is processing input, not beach

## Proposed Solutions

### Solution 1: Non-blocking Event Processing

**Implementation Steps:**

1. **Separate Input Thread**: Move keyboard input handling to a dedicated thread
   ```rust
   // Create a channel for keyboard events
   let (key_tx, key_rx) = mpsc::channel::<Event>(100);
   
   // Spawn input thread
   tokio::spawn(async move {
       loop {
           if event::poll(Duration::from_millis(10))? {
               let event = event::read()?;
               key_tx.send(event).await?;
           }
       }
   });
   ```

2. **Unified Event Loop**: Process all events asynchronously
   ```rust
   loop {
       tokio::select! {
           // Handle keyboard events from channel
           Some(event) = key_rx.recv() => {
               self.handle_event(event).await?;
           }
           
           // Handle server messages
           Some(msg) = self.server_rx.recv() => {
               self.handle_server_message(msg).await?;
           }
           
           // Timeout for periodic updates
           _ = tokio::time::sleep(Duration::from_millis(50)) => {
               self.render_if_needed().await?;
           }
       }
   }
   ```

### Solution 2: Fix Terminal Raw Mode Initialization

**Implementation Steps:**

1. **Verify TTY Before Raw Mode**:
   ```rust
   // Check if we have a proper TTY
   if !std::io::stdin().is_terminal() {
       return Err("Not running in a terminal");
   }
   
   // Enable raw mode with proper error handling
   let mut stdout = stdout();
   terminal::enable_raw_mode()?;
   execute!(stdout, 
       terminal::EnterAlternateScreen,
       cursor::Hide
   )?;
   ```

2. **Ensure Shell Suspension**: 
   - The shell must be suspended when beach takes over the terminal
   - Use proper terminal control sequences to switch between alternate screen buffer
   - Save and restore terminal state correctly

3. **Add Raw Mode Verification**:
   ```rust
   // After enabling raw mode, verify it's working
   let is_raw = terminal::is_raw_mode_enabled()?;
   if !is_raw {
       // Log error and attempt recovery
       debug_log!("Failed to enter raw mode, retrying...");
   }
   ```

### Solution 3: Fix Predictive Echo Replacement

**Implementation Steps:**

1. **Track Predictive Text State**:
   ```rust
   struct PredictiveText {
       content: String,
       position: (u16, u16),
       pending: bool,
   }
   ```

2. **Clear Before Server Echo**:
   ```rust
   fn handle_server_delta(&mut self, delta: Delta) {
       // If we have pending predictive text at this position
       if let Some(pred) = self.predictive_text.take() {
           // Clear the underlined text first
           self.clear_predictive_text(pred.position, pred.content.len());
       }
       
       // Then apply the server delta
       self.apply_delta(delta);
   }
   ```

## Testing Strategy

1. **Unit Tests**: Test event loop with mock stdin/stdout
2. **Integration Tests**: Use PTY to verify proper terminal mode switching
3. **Manual Testing Protocol**:
   - Start server: `./target/debug/beach -v --debug-log /tmp/server.log -- bash`
   - Start client: `./target/debug/beach --join <url> -v --debug-log /tmp/client.log`
   - Type "echo hello" and verify it appears correctly on both sides
   - Type "echo world" on client and verify no duplicate/garbled text

## Implementation Priority

1. **High Priority**: Fix blocking event loop (Solution 1)
   - This is causing immediate usability issues
   - Relatively straightforward to implement

2. **High Priority**: Fix raw mode initialization (Solution 2)
   - Core functionality broken without this
   - May require terminal library updates

3. **Medium Priority**: Fix predictive echo (Solution 3)
   - Quality of life improvement
   - Depends on Solutions 1 & 2 being implemented first

## Related Files

- `/tmp/test-server.log` - Server debug output showing subscription handling
- `/tmp/test-client2.log` - Client debug output showing WebRTC and subscription flow
- `/tmp/beach-road-output.txt` - Signaling server logs
- `apps/beach/src/client/terminal_client.rs` - Main client implementation
- `apps/beach/src/server/terminal.rs` - Server terminal handling
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

## New Issue (2025-09-22): WebRTC Client Display Drift

### Summary

When joining a WebRTC-hosted session (host on `apps/beach`, broker on `apps/beach-road`), the participant’s terminal renders prompts and command output with large leading spaces, stray line-number fragments (`4l`, `2004l`, etc.), and eventually collapses into an empty screen despite the host shell showing perfectly normal output. This occurs immediately after the client prints `Listening for session events…`—no error is raised, but the terminal state is unusable.

### Reproduction

1. Start the session broker:
   ```bash
   BEACH_SESSION_SERVER=http://127.0.0.1:8080 \
   RUST_LOG=debug cargo run -p beach-road
   ```
2. Launch a host session:
   ```bash
   cargo run -p beach -- \
     --session-server http://127.0.0.1:8080 \
     --log-level trace \
     --log-file ~/beach-debug/host.log
   ```
   Example session id: `0d43a35d-d62d-4c15-9ed2-d272c5754a4d`, passcode `807438`.
3. Join from a second terminal:
   ```bash
   BEACH_LOG_FILTER=trace cargo run -p beach -- \
     --session-server http://127.0.0.1:8080 \
     --log-level trace \
     --log-file ~/beach-debug/client.log \
     join 0d43a35d-d62d-4c15-9ed2-d272c5754a4d --passcode 807438
   ```
4. Run simple commands (`echo hi`, `echo world`) on the host.

### Observed Symptoms

- Host terminal looks correct:
  ```
  (base) … % echo hi
  hi
  (base) … % echo world
  world
  (base) … %
  ```
- Client terminal inserts wide gaps and stray fragments:
  ```
  Restored session: Mon Sep 22 11:14:13 EDT 2025
  %
  (base)                  … %            echo hi
  4l
  hi
  (base)                  … %            echo world  ?
  2004l
  world
  ```
- After a few keystrokes the screen degenerates into empty columns; cursor becomes misaligned.

### Supporting Logs

- Client trace: `~/beach-debug/client.log` (see entries around `2025-09-22T15:17:39Z`).
  - Large `Snapshot` payloads arrive with `Row` updates containing `seq: 0` and 80-space `text` fields.
  - Predictive local echo was already flushed (`renderer.clear_all_predictions()` runs when `Grid` frame is processed), yet rows are re-drawn with their previous predictions still visible.
- Host trace: `~/beach-debug/host.log` confirms commands execute normally.

### Initial Hypotheses

- **Row delta ordering:** We are applying `Row` updates that include both `text` and individual `cells`. The row handler populates `pending_predictions`, but the combination of `seq = 0` and empty `cells` may leave stale predictions in the renderer. Verify `UpdateEntry::Row` in `TerminalClient::apply_update`—we always mark predictions even when `seq` has not advanced.
- **Terminal restore sequence:** Broker sends a “Restored session …” banner before our first snapshot. This may include carriage returns that shift the cursor, causing following updates to render at unexpected columns.
- **Local echo mismatch:** Local echo state (`LocalEcho::new()`) may be injecting predictions that never get reconciled because the joiner’s terminal dimensions are different (client defaults to 80x24; host shell might report wider columns). The snapshot shows `Grid` rows=24, cols=80 even when the host is wider.

### Next Steps for Follow-up

1. Instrument `TerminalClient::apply_update` to log every `UpdateEntry::Row` with its `seq` and whether `cells` vs `text` is used. Confirm we clear predictions when a server row arrives.
2. Capture a `TerminalGrid` dump immediately after the distorted prompt appears to compare against the host grid cache.
3. Verify window size negotiation: ensure we propagate the host terminal size through the transporter so the joiner matches (`SpawnConfig::size` vs `TerminalClient::on_resize`).
4. Consider temporarily disabling local echo on the joiner to see if the drift disappears—this would narrow the issue to predictions vs. transport ordering.

This is a blocking usability bug; resolving it should be the next focus once an engineer takes over.

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

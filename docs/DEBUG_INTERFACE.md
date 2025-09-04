# Beach Debug Interface

The Beach debug interface allows you to inspect terminal state remotely through the beach-road session server. This is useful for debugging, monitoring, and understanding what's happening in a shared terminal session.

## Architecture

```
┌──────────────┐      WebSocket      ┌──────────────┐      WebSocket      ┌──────────────┐
│ Debug Client ├────────────────────►│ Beach-Road   ├────────────────────►│ Beach Server │
│ (CLI)        │   Debug Request     │ (Relay)      │   Debug Request     │ (Terminal)   │
│              │◄────────────────────┤              │◄────────────────────┤              │
│              │   Debug Response    │              │   Debug Response    │              │
└──────────────┘                     └──────────────┘                     └──────────────┘
```

## Usage

### 1. Start the Beach-Road Session Server

First, ensure Redis is running:
```bash
redis-server
```

Then start the beach-road server:
```bash
cargo run -p beach-road
```

### 2. Start a Beach Server Session

In another terminal, start a beach server with a command:
```bash
cargo run -p beach -- -- bash
```

This will output a session URL like:
```
Session URL: beach://localhost:8080/abc123def456
```

### 3. Use the Debug CLI

You can now inspect the terminal state using the debug CLI:

#### Get Grid View
```bash
# Get current terminal view
cargo run -p beach-road -- debug --session abc123def456 gridview

# Get view with custom dimensions
cargo run -p beach-road -- debug --session abc123def456 gridview --width 80 --height 24

# Get view with ANSI colors
cargo run -p beach-road -- debug --session abc123def456 gridview --ansi

# Get view starting from specific line
cargo run -p beach-road -- debug --session abc123def456 gridview --from-line 100
```

#### Get Terminal Statistics
```bash
cargo run -p beach-road -- debug --session abc123def456 stats
```

#### Clear Terminal History
```bash
cargo run -p beach-road -- debug --session abc123def456 clear
```

## Output Format

### GridView Output

The GridView command displays the terminal content in a formatted box:

```
╔══════════════════════════════════════════════════════════════╗
║                      TERMINAL GRID VIEW                      ║
╠══════════════════════════════════════════════════════════════╣
║ Dimensions: 80x24
║ Cursor: (5, 10) visible
║ Lines: 0 to 23
║ Timestamp: 2025-01-02T12:34:56Z
╠══════════════════════════════════════════════════════════════╣
║$ ls -la                                                      ║
║total 48                                                      ║
║drwxr-xr-x  12 user  staff   384 Jan  2 12:00 .             ║
║drwxr-xr-x   5 user  staff   160 Jan  1 10:00 ..            ║
║-rw-r--r--   1 user  staff  1234 Jan  2 11:30 README.md     ║
║                                                              ║
╚══════════════════════════════════════════════════════════════╝
```

### Stats Output

```
╔══════════════════════════════════════════════════════════════╗
║                    TERMINAL STATISTICS                       ║
╠══════════════════════════════════════════════════════════════╣
║ History Size: 524288 bytes
║ Total Deltas: 1523
║ Total Snapshots: 15
║ Current Dimensions: 80x24
║ Session Duration: 3600 seconds
╚══════════════════════════════════════════════════════════════╝
```

## Implementation Details

### Debug Message Types

The debug interface uses the following message types defined in `beach-road/src/signaling.rs`:

```rust
pub enum DebugRequest {
    GetGridView {
        width: Option<u16>,
        height: Option<u16>,
        at_time: Option<DateTime<Utc>>,
        from_line: Option<u64>,
    },
    GetStats,
    ClearHistory,
}

pub enum DebugResponse {
    GridView { /* grid data */ },
    Stats { /* statistics */ },
    Success { message: String },
    Error { message: String },
}
```

### Terminal State Tracking

The beach server maintains a complete history of terminal state changes using:
- **Grid**: Current terminal display state
- **GridDelta**: Incremental changes between states
- **GridHistory**: Full history with snapshots for efficient reconstruction
- **GridView**: Interface for querying and re-wrapping terminal content

### Security Considerations

- Debug access requires knowledge of the session ID
- Consider adding authentication/authorization for production use
- Debug interface should be disabled or secured in production environments

## Troubleshooting

### Connection Issues
- Ensure Redis is running: `redis-cli ping`
- Check beach-road server is running on expected port (default: 8080)
- Verify session ID is correct

### No Output
- Ensure the beach server has terminal state tracking enabled
- Check that the session has active terminal content
- Try using `--ansi` flag for colored output

### Performance
- Large terminal histories may take time to transmit
- Use `--from-line` to limit the view to recent content
- Consider clearing history periodically with the `clear` command
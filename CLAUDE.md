# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## CRITICAL: Debugging Guidelines

⚠️ **NEVER write debug output to stdout or stderr** when debugging beach applications!
- The terminal UI will be corrupted if you print to stdout/stderr
- ALWAYS use file-based debug logging with the `--debug-log` flag
- Example: `cargo run -p beach -- --debug-log /tmp/beach-debug.log -- bash`
- Set `BEACH_DEBUG_LOG` environment variable for additional logging
- Any print statements, eprintln!, or println! will break the terminal display

## Development Commands

### Building
```bash
# Build all workspace members
cargo build

# Build with release optimizations
cargo build --release

# Build a specific package
cargo build -p beach
cargo build -p beach-road
```

### Testing
```bash
# Run all tests in workspace
cargo test

# Run tests for a specific package
cargo test -p beach

# Run a specific test
cargo test test_name

# Run tests with output
cargo test -- --nocapture
```

### Checking and Linting
```bash
# Check code compiles without building
cargo check

# Format code
cargo fmt

# Run clippy linter
cargo clippy
```

## Architecture Overview

This is a Rust workspace containing two applications for terminal sharing functionality:

### Core Components

1. **beach** (`apps/beach/`) - Main terminal sharing application
   - **Client/Server Architecture**: Can run in either client mode (with `--join`) or server mode (without `--join`)
   - **Transport Layer** (`src/transport/`): Abstraction for network communication
     - WebRTC implementation for peer-to-peer connectivity
     - Trait-based design allowing multiple transport implementations
   - **Session Management** (`src/session/`): Handles session URLs, IDs, and state
     - `Session`: Core session with transport and passphrase
     - `ServerSession`: Server-specific session tracking connected clients
     - `ClientSession`: Client-specific session with unique instance ID
   - **Server** (`src/server/`): Terminal server that executes commands
   - **Client** (`src/client/`): Terminal client that connects to servers

2. **beach-road** (`apps/beach-road/`) - Auxiliary application (currently placeholder)

### Key Design Patterns

- **Trait-based Transport**: The `Transport` trait allows swapping network implementations (WebRTC, WebSocket, etc.)
- **Generic Sessions**: Sessions are parameterized by transport type, enabling compile-time transport selection
- **Async/Await**: Uses Tokio for async runtime
- **Command Pattern**: Server accepts commands via CLI args after `--`

### Entry Points

- **Server Mode**: `beach -- <command>` starts a server running the specified command
- **Client Mode**: `beach --join <session-url>` connects to an existing session
- Optional `--passphrase` flag for encryption in both modes

## Terminal Content Caching Architecture

Beach uses a viewport-based subscription model for efficient terminal content delivery:

### How Terminal Caching Works

1. **The Grid IS the Cache**: The `GridRenderer.grid` field stores ALL terminal content (both current and historical)
2. **Viewport Subscriptions**: Client requests specific line ranges from server based on scroll position
3. **SnapshotRange Messages**: Server responds with `SnapshotRange` containing the requested historical data
4. **Single Source of Truth**: All rendering uses `grid.cells` - there are no separate caches

### Data Flow for Scrollback

1. User scrolls up → `scroll_offset` increases
2. Client calculates needed viewport and sends `ViewportChanged` message to server
3. Server retrieves historical data and sends back `SnapshotRange`
4. Client calls `apply_snapshot()` to populate `grid` with historical content
5. Renderer uses `scroll_offset` to determine which rows from `grid` to display

**Important**: The `history_cache` field is deprecated and unused. All rendering accesses `grid.cells` directly.

## Configuration

Beach supports configuration through environment variables:

### Environment Variables

- `BEACH_SESSION_SERVER`: The session server address (defaults to `localhost`)
  - Example: `BEACH_SESSION_SERVER=custom-server.example.com beach server`
  - Used in server mode to specify which session server to connect to
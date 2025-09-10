# Scrollback History Implementation Specification

## Problem Statement

The beach terminal sharing application currently has a fundamental limitation: it only synchronizes the visible terminal viewport between server and client, not the scrollback history. This means clients can only see what's currently visible in the server's terminal window (typically 24-48 rows), even though the server's terminal emulator may have thousands of lines of scrollback history.

### Current Behavior

1. **Server Side**: The server terminal (using alacritty_terminal) maintains only the visible grid (e.g., 80x24 cells)
2. **Protocol**: Only transmits Grid snapshots and deltas for the visible viewport
3. **Client Side**: Receives and displays only what's in the current viewport
4. **User Impact**: When running applications that produce lots of output (like Claude Code), users cannot scroll up to see previous output that has scrolled off screen

### Example Scenario

When a user runs Claude Code in the server terminal:
- Claude Code outputs 100+ lines of text
- Server terminal shows only the last 24 lines
- Client receives only those 24 visible lines
- User attempts to scroll up but can only move 1-2 rows within the 24-line grid
- Historical output is completely inaccessible to the client

## Technical Analysis

### Current Architecture

```
Server Terminal State
├── AlacrittyTerminal (80x24 visible grid)
│   └── Grid { cells: Vec<Vec<Cell>> }  // Only visible rows
├── TrackerDataSource
│   └── Tracks changes to visible grid only
└── SubscriptionHub
    └── Sends Grid snapshots (24 rows) to clients

Client Terminal
├── GridRenderer
│   ├── grid: Grid (24 rows from server)
│   └── scroll_offset: u16 (limited by grid size)
└── Can only scroll within received grid
```

### Key Components Affected

1. **`apps/beach/src/server/terminal_state/`**
   - Backend already maintains Grid + GridHistory with line/time indexes
   - Need retention policy + view derivation wired for scrollback UX

2. **`apps/beach/src/protocol/subscription/`**
   - `messages.rs`: Grid/GridDelta messages don't include history
   - No protocol messages for requesting historical data

3. **`apps/beach/src/client/`**
   - `grid_renderer.rs`: Only stores single Grid snapshot
   - `terminal_client.rs`: No mechanism to request history

## Proposed Solution

### Design Alignment (reuse existing primitives)

Leverage the existing GridHistory + GridView on the server. Scrollback is
modeled as requesting a different viewport (view mode + position) derived
from GridHistory, not a separate history store.

1. **Server retention**: Bound GridHistory by lines or memory
2. **Protocol reuse**: Use ModifySubscription with ViewMode::Historical and
   ViewPosition { line | time }
3. **Client cache**: Optionally cache recent historical viewports
4. **Smart sync**: Fetch historical snapshots on demand during scrolling

### Detailed Implementation Plan

#### Phase 1: Server-Side Retention Policy

```rust
pub struct HistoryBuffer {
    /// Circular buffer of historical lines
    lines: VecDeque<HistoryLine>,
    /// Maximum lines to store (configurable, default 10,000)
    max_lines: usize,
    /// Line counter for absolute positioning
    line_counter: LineCounter,
}

pub struct HistoryLine {
    /// The actual cell content
    cells: Vec<Cell>,
    /// Absolute line number in terminal history
    line_number: u64,
    /// Timestamp when line was completed
    timestamp: DateTime<Utc>,
}
```

**Integration with Terminal Backend**:
- Ensure deltas and snapshots feed GridHistory with line/time indices
- Provide derive_from_line/derive_at_time helpers for viewports

#### Phase 2: Protocol (reuse + optional metadata)

Use existing subscription messages:
- Client → Server: ModifySubscription { subscription_id, dimensions?, mode: Some(Historical), position: Some(ViewPosition { line | time }) }
- Server → Client: Snapshot for requested viewport

Optional: a small HistoryInfo message (server→client) to inform scroll limits
(oldest_line, latest_line, total_lines). This can be included in
SubscriptionAck or sent periodically.

#### Phase 3: Client-Side View Management

**Enhanced GridRenderer** in `apps/beach/src/client/grid_renderer.rs`:

```rust
pub struct GridRenderer {
    // Existing fields...
    
    /// History cache (lines above current grid)
    history_cache: BTreeMap<u64, HistoryLine>,
    /// Pending history requests
    pending_requests: HashSet<String>,
    /// History metadata from server
    history_metadata: Option<HistoryMetadata>,
}

impl GridRenderer {
    /// Calculate what history needs to be fetched for current scroll
    pub fn calculate_history_needs(&self) -> Option<HistoryRequest> {
        // Determine if we need to fetch history based on scroll position
        // Return request for missing lines
    }
    
    /// Render combined history + current grid
    pub fn render_with_history(&self) -> Text {
        // Merge history cache with current grid
        // Handle gaps in history gracefully
    }
}
```

#### Phase 4: Smart Synchronization

**Optimization Strategies**:

1. **Predictive Fetching**: Request history in chunks before user scrolls to them
2. **Caching**: Keep recently viewed history in memory
3. **Compression**: Use existing chunking/compression for large Snapshots (already in transport)
4. **Delta Updates**: Historical views change rarely; Snapshots suffice; live view continues with Deltas

### Channel Routing (WebRTC dual-channel)

- Snapshots (including historical viewports): reliable channel to guarantee integrity
- Live Deltas: output/unreliable channel to avoid head‑of‑line blocking

### Migration Path

1. **Phase 1**: Implement without breaking changes
   - Add history buffer to server (dormant initially)
   - Deploy new server version

2. **Phase 2**: Protocol additions
   - Add new message types
   - Old clients ignore new messages
   - New clients detect server capabilities

3. **Phase 3**: Client implementation
   - New clients use history when available
   - Graceful fallback for old servers

4. **Phase 4**: Optimization
   - Monitor performance
   - Tune buffer sizes and fetch strategies

## Alternative Approaches Considered

### 1. Full Terminal Multiplexer Mode
- Implement full tmux-like functionality
- Pros: Complete feature parity with terminal multiplexers
- Cons: Massive scope increase, complexity

### 2. Unlimited Grid Size
- Send entire terminal output as one huge grid
- Pros: Simple conceptually
- Cons: Memory issues, inefficient transfers, poor performance

### 3. External History Storage
- Store history in separate file/database
- Pros: Unlimited history, persistent across sessions
- Cons: Complexity, sync issues, storage management

## Implementation Checklist

- [ ] Ensure GridHistory retention is enabled and bounded by config
- [ ] Expose derive_from_line/derive_at_time via subscription handler
- [ ] Implement server-side ModifySubscription for historical viewports
- [ ] Update SubscriptionHub to handle history requests
- [ ] Enhance GridRenderer with historical view switching + small cache
- [ ] Implement client-side view request logic (ModifySubscription)
- [ ] Add scroll position synchronization
- [ ] Implement predictive fetching
- [ ] Add compression for history transfers
- [ ] Write comprehensive tests
- [ ] Update documentation
- [ ] Performance benchmarking

## Configuration

New configuration options needed:

```toml
[server]
# Maximum lines of history to keep (0 = disabled)
max_history_lines = 10000

[client]
# History cache size in lines
history_cache_size = 1000
# Predictive fetch distance (lines)
prefetch_distance = 100
```

## Performance Considerations

### Memory Usage
- Server: bounded by GridHistory retention policy (lines or memory). Use periodic snapshotting + compaction.
- Client: cache a limited number of historical viewports.

### Network Usage
- Historical snapshots are sent on demand; transport chunking handles large payloads; predictive window should be modest (e.g., 100–200 lines).

### Latency
- Target: <50ms for history fetch on local network
- <200ms for remote connections

## Testing Requirements

### Unit Tests
- HistoryBuffer circular buffer logic
- History request/response serialization
- Cache management algorithms

### Integration Tests
- Server captures history correctly
- Client fetches and displays history
- Scroll synchronization works
- Performance under load

### End-to-End Tests
- Run command with large output
- Verify client can scroll through entire history
- Test with multiple clients
- Verify cleanup on disconnect

## Security Considerations

1. **Memory Limits**: Prevent DoS by limiting history size
2. **Rate Limiting**: Limit history request frequency
3. **Access Control**: Ensure clients can only access their session's history
4. **Data Sanitization**: Clean control sequences from history

## Future Enhancements

1. **Persistent History**: Save history to disk for session recovery
2. **Search**: Full-text search through history
3. **Annotations**: Mark important lines in history
4. **Export**: Export history to file
5. **Compression**: Advanced compression algorithms
6. **Streaming**: Stream history as it's generated

## Dependencies

- No new external dependencies required
- Uses existing VecDeque, BTreeMap from std
- Leverages existing protocol infrastructure

## Timeline Estimate

- Phase 1 (Server History): 2-3 days
- Phase 2 (Protocol): 1-2 days
- Phase 3 (Client): 2-3 days
- Phase 4 (Optimization): 2-3 days
- Testing & Documentation: 2-3 days

**Total: 2-3 weeks for full implementation**

## Conclusion

This specification outlines a comprehensive solution to add scrollback history support to beach. The phased approach ensures backward compatibility while progressively enhancing functionality. The design balances performance, memory usage, and user experience to provide seamless access to terminal history across the network.

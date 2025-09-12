# Single GridView Subscription (Viewport-Driven) Spec

Status: Draft v1

Owner: beach

Scope: `apps/beach` (client, server, protocol, subscription hub, terminal state), interoperating with current WebRTC transport.

Summary: Replace the current dual-mode subscription model (Realtime vs Historical) with a single viewport-driven subscription. The client always has one subscription and continuously reports its viewable row range; the server prioritizes streaming snapshots and deltas for that range first, then fills nearby rows, then background. Ordering is guaranteed via a global sequence and snapshot watermarks. Snapshots go over the reliable channel; deltas may go over the unreliable output channel.

---

## Goals

- One subscription type; remove client-side mode switching and anchor state.
- Fast perceived scrolling: placeholders immediately, rows around viewport filled first.
- Correctness under loss/reordering: snapshot watermark + global delta sequence.
- Bounded memory and bandwidth: prioritize and coalesce; avoid “send everything”.
- Incremental migration: maintain backward compatibility while landing server and client changes.

## Non-Goals

- Persisting history across process restarts.
- Multi-view per client; this spec targets a single active viewport per subscription.
- Perfect reconstruction of every intermediate delta outside the client’s viewport.

---

## Key Concepts

- Absolute line numbers: The terminal history is indexed by an absolute, monotonically increasing line counter (already present in Grid.start_line/end_line via `LineCounter`).
- Viewport: The client’s viewable line range [start_line, end_line], inclusive, plus a prefetch margin (before/after) for smooth scrolling.
- Watermark sequence: A `watermark_seq` included in each snapshot chunk indicating “this snapshot reflects deltas ≤ watermark”. Any deltas with `sequence > watermark_seq` must be applied after the snapshot.
- Channel semantics: Snapshots (reliable); deltas (unreliable) when possible.

---

## Protocol Changes

Files to update:

- apps/beach/src/protocol/subscription/messages.rs:1
- apps/beach/src/protocol/subscription/client_messages.rs:1
- apps/beach/src/protocol/subscription/server_messages.rs:1

### New/Updated Types

1) Add `Viewport` and `Prefetch`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Viewport {
    pub start_line: u64,  // inclusive
    pub end_line: u64,    // inclusive
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Prefetch {
    pub before: u32,  // lines before viewport to prioritize
    pub after: u32,   // lines after viewport to prioritize
}
```

2) Deprecate `ViewMode` and `ViewPosition` for subscriptions. Keep them to deserialize old clients, but mark as deprecated in docstrings and avoid new usages. Map legacy `ModifySubscription { mode, position }` into an equivalent `ViewportChanged` internally (see Migration).

3) Modify `ClientMessage`:

```rust
pub enum ClientMessage {
    // Existing variant updated (mode/position deprecated)
    Subscribe {
        subscription_id: String,
        dimensions: Dimensions,      // width is authoritative; height is viewport height
        #[serde(skip_serializing_if = "Option::is_none")]
        viewport: Option<Viewport>,  // if absent, server assumes tail
        #[serde(skip_serializing_if = "Option::is_none")]
        prefetch: Option<Prefetch>,  // default { before: 100, after: 100 }
        #[serde(skip_serializing_if = "Option::is_none")]
        follow_tail: Option<bool>,   // default true when viewport near latest

        // DEPRECATED: kept for compatibility
        #[serde(skip_serializing_if = "Option::is_none")]
        mode: Option<ViewMode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        position: Option<ViewPosition>,

        #[serde(skip_serializing_if = "Option::is_none")]
        compression: Option<CompressionType>,
    },

    // New: fast updates on scroll (can be sent over unreliable channel via transport)
    ViewportChanged {
        subscription_id: String,
        viewport: Viewport,
        #[serde(skip_serializing_if = "Option::is_none")]
        prefetch: Option<Prefetch>,
        #[serde(skip_serializing_if = "Option::is_none")]
        follow_tail: Option<bool>,
    },

    // Legacy path retained but mapped server-side to viewport (see Migration)
    ModifySubscription { /* unchanged signature for compatibility */ },
    // ...existing variants
}
```

4) Add `SnapshotRange` and `watermark_seq` to server messages:

```rust
pub enum ServerMessage {
    // Full-snapshot for a specific line range (chunked as needed)
    SnapshotRange {
        subscription_id: String,
        sequence: u64,         // message sequence to keep increasing per-subscription
        watermark_seq: u64,    // all deltas ≤ watermark are reflected in `grid`
        grid: Grid,            // width = subscription.width, height = range size
        timestamp: i64,
        checksum: u32,
    },

    // Existing delta semantics; ensure global monotonic `sequence` per subscription
    Delta { subscription_id: String, sequence: u64, changes: GridDelta, timestamp: i64 },

    // Kept for compatibility with initial snapshot (server may emit SnapshotRange instead)
    Snapshot { /* unchanged but recommend deprecating in favor of SnapshotRange */ },

    // History metadata (already present)
    HistoryInfo { /* unchanged */ },
    // ...existing variants
}
```

5) Optional: extend `HistoryInfo` to carry `retained_lines_max` if desired. Not required.

### Ordering and Idempotency

- Deltas: carry `sequence` (already present). Server guarantees monotonic per-subscription sequences.
- SnapshotRange: carry `watermark_seq`. Client applies snapshot, buffers deltas with `sequence > watermark_seq` for that range, then replays them.
- Client keeps `last_applied_seq` per subscription (and optionally per row) to skip stale updates.

---

## Server Changes

Files to update:

- apps/beach/src/subscription/hub.rs:1
- apps/beach/src/server/terminal_state/data_source_impl.rs:1
- apps/beach/src/server/terminal_state/grid_view.rs:1 (no breaking API; use existing helpers)
- apps/beach/src/server/mod.rs:1 (message handling and channel routing)

### Per-Subscription State

Augment the `Subscription` struct in `hub.rs` to support viewport streaming:

```rust
struct Subscription {
    // existing fields ...
    viewport: Option<Viewport>,   // current viewport
    prefetch: Prefetch,           // default {100, 100}
    follow_tail: bool,            // server hint to prioritize tail deltas
    watermark_seq: u64,           // last snapshot watermark sent
    covered_ranges: Vec<(u64,u64)>, // ranges already snapshotted to client
}
```

Migration default: infer `viewport` from dimensions height and server latest_line (tail), set `prefetch = {100,100}`, `follow_tail = true`.

### Channel Selection

Update `SubscriptionHub::send_to_subscription` to choose channel by message type:

- SnapshotRange/Snapshot/HistoryInfo/SubscriptionAck/Error → Control (reliable)
- Delta/DeltaBatch → Output (unreliable) when `transport.supports_multi_channel()`; otherwise fall back to Control.

Implementation: add a sibling helper `send_to_subscription_on(purpose, message)` using `transport.channel(ChannelPurpose::...)`.

### Viewport Update Handling

- Handle `ClientMessage::ViewportChanged` in `SubscriptionHub::handle_incoming`:
  - Update `subscription.viewport/prefetch/follow_tail`.
  - Invalidate or reprioritize queued work (cancel in-flight background chunks).
  - Immediately schedule high-priority snapshot chunks for the viewport and margins not yet covered.

- For legacy `ModifySubscription { mode, position }`, map as:
  - If `mode == Historical && position.line = L`: compute viewport `{ start = L, end = L + dims.height - 1 }`; set `follow_tail = false`.
  - If `mode == Realtime`: compute viewport near tail `{ end = latest_line, start = end - dims.height + 1 }`; set `follow_tail = true`.

### Prioritized Streaming Loop

Replace the current broadcast-only `start_streaming()` behavior with per-subscription prioritization while keeping a single loop over `next_delta()`:

1) Maintain a per-subscription priority queue of send tasks:
   - P0: SnapshotRange chunks for viewport lines plus +/- prefetch margin.
   - P1: Deltas that affect cells inside viewport.
   - P2: SnapshotRange chunks outside viewport not yet sent (excluding the margin already covered by P0).
   - P3: Deltas outside viewport for rows already snapshotted to this client.

2) On delta arrival (`next_delta()`): determine affected absolute rows using the terminal tracker/state; enqueue P1/P3 tasks depending on whether the row is within the client’s materialized ranges.

3) On viewport change: drop lower-priority queued tasks, promote P0 tasks for the new viewport first, then refill P2.

4) Backpressure: cap background throughput (e.g., only send one P2 chunk per N ms when deltas are flowing).

Implementation notes:

- The server already has history derivation in `grid_view.rs`. Use `GridView::derive_from_line(line, Some(height))` to produce a `Grid` for any [start_line, end_line] window.
- Add to the data source a helper to get both a grid and the latest sequence (for watermark):

```rust
#[async_trait]
pub trait TerminalDataSource {
    async fn snapshot(&self, dims: Dimensions) -> Result<Grid>;
    async fn snapshot_with_view(&self, dims: Dimensions, /* deprecated */ mode: ViewMode, position: Option<ViewPosition>) -> Result<Grid>;

    // New: get grid for a start line and row_count + the latest delta sequence as watermark
    async fn snapshot_range_with_watermark(&self, width: u16, start_line: u64, rows: u16) -> Result<(Grid, u64)>;

    async fn next_delta(&self) -> Result<GridDelta>;
    async fn invalidate(&self) -> Result<()>;
    async fn get_history_metadata(&self) -> Result<HistoryMetadata>;
}
```

- Implementation in `apps/beach/src/server/terminal_state/data_source_impl.rs`: call `GridView::derive_from_line(start_line, Some(rows))` and get the current delta sequence from the tracker/history (if not available, add a `current_sequence()` accessor on the tracker that mirrors the last delta sequence emitted).

### Initial Subscribe Flow

On `Subscribe`:

1) Send `SubscriptionAck`.
2) Send `HistoryInfo { oldest_line, latest_line, total_lines, ... }` immediately.
3) Initialize subscription state: viewport (tail if none provided), prefetch, follow_tail.
4) Schedule P0 tasks to cover viewport +/- margin as `SnapshotRange` with `watermark_seq`.

### Delta Flow

- Keep `GridDelta` semantics but ensure `sequence` is global monotonic per subscription.
- For clients with open `Output` channel, send deltas there using the same envelope (`AppMessage::Protocol`). Otherwise, send over Control.

---

## Client Changes

Files to update:

- apps/beach/src/client/terminal_client.rs:1
- apps/beach/src/client/grid_renderer.rs:1

### TerminalClient

- On connect in `connect_and_subscribe` (apps/beach/src/client/terminal_client.rs:588):
  - Replace `Subscribe { mode: ViewMode::Realtime, position: None, ... }` with:
    - `Subscribe { viewport: Some(tail_window), prefetch: Some({before:100, after:100}), follow_tail: Some(true) }`.
    - Tail window: use `HistoryInfo.latest_line` once received; until then, optimistically request `{ end = unknown → request height as snapshot and rely on first SnapshotRange }`.

- Handle new messages in `handle_server_message`:
  - `HistoryInfo`: initialize `RowCache` length (see below) with placeholders; set `latest_line/oldest_line`.
  - `SnapshotRange { watermark_seq, grid, ... }`: write rows into `RowCache` for `[grid.start_line..grid.end_line]`, mark them materialized with the associated `watermark_seq`.
  - `Delta { sequence, changes, ... }`: apply if delta row is within materialized ranges; otherwise buffer or drop (we avoid applying deltas to unknown rows).

- On scroll in `handle_mouse_event`/`handle_key_event` (replace current `ModifySubscription`/ViewMode logic around apps/beach/src/client/terminal_client.rs:1740):
  - Compute new `Viewport` from scroll position and history metadata.
  - Send `ViewportChanged` over the unreliable channel if available; otherwise via Control.
  - Set `follow_tail = true` only when the viewport ends within the last M lines (e.g., M = 2 × screen height).

### GridRenderer → RowCache

Augment/replace `GridRenderer` to keep a bounded row cache and placeholders:

- Add `RowCache` structure holding:
  - `latest_line`, `oldest_line` (from `HistoryInfo`).
  - Bounded in-memory storage for lines (e.g., LRU or sliding window of N lines around viewport; configurable).
  - `materialized: Vec<Range<u64>>` or bitmap to track which rows have real content.
  - `per_row_seq: HashMap<u64, u64>` optional, last applied sequence per row.

- When `HistoryInfo` arrives: initialize cache length to `min(total_lines, client_cache_cap)`; mark all lines as not materialized and fill UI with placeholders (e.g., ‘·’ or ‘⌛’ character) for rows not present.

- `apply_snapshot_range(grid, watermark_seq)`: fill cache rows, set `per_row_seq[row] = watermark_seq` for filled rows.

- `apply_delta(delta)`: if a change affects `row_abs` that’s materialized and `sequence > per_row_seq[row_abs]`, apply and update `per_row_seq`.

- Rendering: use the cache to render the viewport, showing placeholders for not-yet materialized rows.

Note: For v1, we can keep using `Grid` for rendering and emulate placeholders by pre-initializing the `Grid` cells with a placeholder char before applying snapshot rows. The full RowCache can be introduced incrementally.

### Remove ViewMode Flow

- Remove calls to `enter_historical_mode` / `enter_realtime_mode` and `ModifySubscription` toggles in `terminal_client.rs` and `grid_renderer.rs`. Replace with viewport math + `ViewportChanged` messages.

---

## Transport Integration

Files:

- apps/beach/src/transport/channel.rs:1
- apps/beach/src/session/mod.rs:496
- apps/beach/src/subscription/hub.rs:1182 (send_to_subscription routing)

Behavior:

- Keep Control channel (reliable) for `SubscriptionAck`, `HistoryInfo`, `SnapshotRange`, and errors.
- Prefer Output channel (unreliable) for `Delta` messages; fallback to Control if Output not available.
- Clients may send `ViewportChanged` over Output if present for low latency; server should accept from either channel.

---

## Migration & Compatibility

- Protocol: Keep `ViewMode`, `ViewPosition`, `ModifySubscription` to parse old clients. Map `ModifySubscription` on the server to viewport semantics as described (historical → explicit viewport; realtime → tail).
- Server: Continue to accept `Snapshot` for initial responses, but prefer `SnapshotRange` internally. Old clients will still work.
- Client: Implement new flow; when talking to old servers (no `SnapshotRange`), handle `Snapshot` as a full-viewport snapshot and skip watermark logic.

---

## Testing Plan

Update and add tests in `apps/beach/src/tests/`.

1) Protocol serialization

- File: apps/beach/src/tests/protocol_test.rs:1
  - Add round-trip tests for `Viewport`, `Prefetch`, `ClientMessage::ViewportChanged`, and `ServerMessage::SnapshotRange`.
  - Keep existing tests using `ViewMode` but mark as legacy; ensure de/serialization still works.

2) Hub scheduling

- New tests (add under `apps/beach/src/tests/server/`):
  - Setup a `SubscriptionHub` with a mock data source that can produce `snapshot_range_with_watermark` and deltas with increasing sequences.
  - Assert priority: upon `ViewportChanged`, the first sends are `SnapshotRange` covering viewport, then margin; background chunks are delayed when deltas are flowing.
  - Verify channel selection by inspecting which channel the mock transport received messages on (Control for snapshots, Output for deltas).

3) Client viewport updates

- File: apps/beach/src/tests/client/phase2a_test.rs:1
  - Replace/augment `test_scroll_prefetch` to assert that a scroll triggers a `ViewportChanged` message (using a mock send queue), not `ModifySubscription`.
  - Add a test that applies `HistoryInfo`, then `SnapshotRange` for a window, and confirms placeholders get replaced.

4) Ordering correctness

- Add a test where a `SnapshotRange` with `watermark_seq = N` arrives, then deltas with `sequence > N`: verify apply order and idempotency.

---

## Step-by-Step Implementation Checklist

1) Protocol
- [ ] Add `Viewport`, `Prefetch` types; update `ClientMessage::Subscribe` and introduce `ViewportChanged`.
- [ ] Add `ServerMessage::SnapshotRange { watermark_seq, ... }`.
- [ ] Keep `ViewMode`/`ViewPosition` in place but mark deprecated in comments.

2) Server data source
- [ ] Add `snapshot_range_with_watermark(width, start_line, rows)` to `TerminalDataSource`.
- [ ] Implement in `TrackerDataSource` using `GridView::derive_from_line` and a `current_sequence()` accessor.

3) SubscriptionHub
- [ ] Extend `Subscription` state with viewport/prefetch/follow_tail/covered_ranges/watermark_seq.
- [ ] Add `handle_incoming` handler for `ViewportChanged`.
- [ ] Implement per-subscription prioritized queue; on deltas, schedule P1/P3; on viewport change, (re)queue P0, then P2.
- [ ] Route messages by channel purpose (Control vs Output).

4) Server entry
- [ ] Ensure `TerminalServer` wires debug/logging and starts streaming unchanged; hub does prioritization.

5) Client
- [ ] Update `connect_and_subscribe` to use viewport subscription and handle `HistoryInfo` + `SnapshotRange`.
- [ ] Replace `ModifySubscription`/ViewMode logic with `ViewportChanged` on scroll.
- [ ] Add RowCache or placeholder-backed rendering; apply ordering via `watermark_seq` and `sequence`.

6) Tests
- [ ] Update `protocol_test.rs` for new messages.
- [ ] Add hub scheduling tests and client viewport tests.

7) Docs
- [ ] Update `docs/SCROLLBACK_HISTORY_SPEC.md` to reference this unified model.
- [ ] Note channel routing expectations in `docs/dual-channel-webrtc-spec.md`.

---

## Implementation Notes and Hints

- History line math: clamp viewports within `[oldest_line, latest_line]`. When at tail (`end_line` within last `M` lines), set `follow_tail = true` to bias server scheduling for real-time deltas.
- Chunk sizing: for `SnapshotRange`, prefer chunks of ~viewport height or fixed 200 rows to balance latency and throughput; keep simple for v1.
- Watermarks: if computing a strict “last sequence included in snapshot” is hard initially, use “last seen delta sequence” at the time of snapshot generation; it’s sufficient for client ordering.
- Out-of-viewport deltas: only send for rows already snapshotted to the client to avoid meaningless updates.
- Backpressure: do not let background snapshotting starve deltas; cap background bandwidth.

---

## File Touchpoints (Quick Reference)

- Protocol
  - apps/beach/src/protocol/subscription/messages.rs:1
  - apps/beach/src/protocol/subscription/client_messages.rs:1
  - apps/beach/src/protocol/subscription/server_messages.rs:1

- Server
  - apps/beach/src/server/terminal_state/data_source_impl.rs:1
  - apps/beach/src/subscription/hub.rs:1
  - apps/beach/src/server/mod.rs:1

- Client
  - apps/beach/src/client/terminal_client.rs:588
  - apps/beach/src/client/terminal_client.rs:1740
  - apps/beach/src/client/grid_renderer.rs:1

- Tests
  - apps/beach/src/tests/protocol_test.rs:1
  - apps/beach/src/tests/client/phase2a_test.rs:1
  - apps/beach/src/tests/server/ (new)

---

## Rollout Plan

1) Land protocol additions behind backwards-compatible serde (retain old fields).
2) Server: implement `SnapshotRange` + `ViewportChanged` handling and channel routing; continue sending legacy `Snapshot` to old clients.
3) Client: ship viewport-driven behavior gated by server capability detection (presence of `SnapshotRange`).
4) Remove legacy ViewMode toggles once both ends are migrated; keep deserialization for safety for one release.

This plan yields a simpler client (no modes), a single subscription model, and localized server complexity (prioritization, ordering). It aligns with our dual-channel transport and existing terminal history primitives.


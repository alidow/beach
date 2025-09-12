# Server‑Side Scrollback Implementation Plan

This document describes how to implement reliable historical scrollback for the Beach terminal session, using the server’s snapshot + delta history as the source of truth. The goal is for a client that requests a historical view (e.g., when the user scrolls up) to receive a non‑blank grid that truly contains the requested lines, without introducing client‑side history buffers or ad‑hoc logging.

## Goals

- Serve historical grids by line number (and later by time) from the existing history system.
- Keep architecture clean: server owns history; client requests views.
- Preserve realtime bottom anchoring; define clear top‑anchored semantics for historical views.
- Avoid new env vars or stdout/stderr logging; re‑use existing debug recorder if needed for verification.

## Current Architecture (high‑level)

- Server history is maintained in `GridHistory` as an initial grid plus an ordered map of deltas, plus snapshots:
  - File: `apps/beach/src/server/terminal_state/grid_history.rs`
  - Key methods: `add_delta`, `add_snapshot`, `reconstruct_from_sequence`, `get_current`.
- View derivation is implemented in `GridView`:
  - File: `apps/beach/src/server/terminal_state/grid_view.rs`
  - Key methods: `derive_realtime(max_height)`, `derive_from_line(line_num, max_height)`.
- Transport‑agnostic data source (`TrackerDataSource`) already uses `GridView` to produce snapshots for given modes:
  - File: `apps/beach/src/server/terminal_state/data_source_impl.rs`
  - Key method: `snapshot_with_view(dims, mode, position)`.
- Subscription pipeline (`SubscriptionHub`) uses the data source to send snapshots/deltas to clients.

## Problems Today

- `GridHistory::get_from_line(line)` is effectively a stub; it returns the current grid instead of reconstructing historical content.
- `GridView::derive_from_line()` slices from the current grid rather than asking history, so it cannot provide actual scrollback.
- Snapshots exist frequently (good), but no fast index is used to find the snapshot that actually contained a requested line.

Notes on current snapshots
- The snapshots we take capture the visible viewport at the time of snapshot (e.g., ~24 rows). This is expected and sufficient for scrollback so long as snapshots are frequent: every historical line should have been visible in at least one snapshot as it scrolled through the viewport. We do not need to store an unbounded scrollback buffer in each snapshot; we only need to locate a snapshot taken while the target line was visible, then reconstruct that frame.
- If snapshot cadence is reduced in the future, ensure the cadence still guarantees that every line passes through at least one snapshot (e.g., snapshot at most a few deltas apart during output bursts).

## Proposed Design

### 1) Snapshot Metadata Index in `GridHistory`

Add a compact index to quickly find the best snapshot for a requested line number.

- File: `apps/beach/src/server/terminal_state/grid_history.rs`
- New struct:
  ```rust
  struct SnapshotMeta {
      seq: u64,
      start_line: LineCounter,
      end_line: LineCounter,
      timestamp: DateTime<Utc>,
  }
  ```
- New field on `GridHistory`:
  ```rust
  snapshot_meta: Vec<SnapshotMeta>, // append-only, same order as snapshots
  ```
- Populate `snapshot_meta` whenever `add_snapshot(grid: Grid)` is called:
  - `start_line = grid.start_line`
  - `end_line = grid.end_line` (or `start_line + grid.height - 1`)
  - `timestamp = grid.timestamp`
  - `seq = current_sequence`
  - `snapshot_meta.push(SnapshotMeta { ... })`
- Memory management:
  - When enforcing memory limits and evicting snapshots (see `enforce_memory_limits`), also drop corresponding `snapshot_meta` entries so the index remains consistent. Keep data structures in sync; do not leave dangling indices.

Why a vector? The number of snapshots is modest (we already snapshot frequently). Linear scan or binary search over a vector is fine and keeps code simple. If needed later, we can move to a `BTreeMap<u64, SnapshotMeta>` keyed by sequence.

### 2) Implement `GridHistory::get_from_line(line)`

- File: `apps/beach/src/server/terminal_state/grid_history.rs`
- Replace the stub with logic that returns a grid whose `[start_line..=end_line]` contains `line`.
- Algorithm:
  1. Convert `line` to `LineCounter`.
  2. Find a snapshot whose visible window contains that line:
     - Binary search `snapshot_meta` for the last meta with `start_line <= line`.
     - If that meta also has `end_line >= line`, that snapshot contains the line; set `seq_start = meta.seq`.
     - Otherwise, try nearby metas (e.g., a few forward/backward steps) to account for edges.
     - If no meta is found (line older than earliest), fall back to the earliest snapshot (or return a clean, typed error). Avoid falling back to the current grid for historical requests, as that misrepresents the requested state and can produce apparent blanks.
  3. Reconstruct the grid at `seq_start` using `reconstruct_from_sequence(seq_start)`. This returns the terminal frame at that snapshot.
  4. If the reconstructed window still does not contain `line` (rare):
     - Option A (simple): choose a later meta (if any) whose start is closer to `line` and reconstruct there.
     - Option B (fine‑grained): iterate deltas from `seq_start+1` forward, applying them and checking `start_line..=end_line` each step until `line` is in range or we hit current. This guarantees we return a frame that actually contained the target line.
  5. Return the reconstructed `Grid`.

Notes:
- We are reconstructing a *frame* from when the target line was visible, not synthesizing a larger scrollback window. This matches the terminal’s semantics and our existing model.
- This method should not print/log; rely on existing debug recorder events if needed during internal testing.

### 3) Make `GridView::derive_from_line()` use history

- File: `apps/beach/src/server/terminal_state/grid_view.rs`
- Replace the current slice‑from‑current logic with a call to history:
  ```rust
  let history = self.history.lock().unwrap();
  let historical_grid = history.get_from_line(line_num)?;
  ```
- View semantics:
  - For historical views, treat `line_num` as the logical top of the returned viewport whenever possible.
  - If `max_height` is specified and smaller than the grid height, slice to `height` rows starting from `line_num` (top‑anchored), not bottom‑aligned.
  - For realtime views, keep existing bottom alignment (unchanged) so the latest output shows at the bottom of the screen.
- Implementation detail:
  - Do not change `truncate_to_height`’s realtime bottom‑alignment behavior. Instead, in `derive_from_line`, apply a top‑anchored slice directly over `historical_grid`:
    - Compute `row_offset = line_num - historical_grid.start_line`
    - Copy `height` rows starting from `row_offset` (clamp to not exceed grid bounds)
    - Preserve cursor; if cursor is above the new top, mark `visible=false` in the returned view

Cursor & bounds notes
- If the requested `line_num` is within a few rows of the grid’s end (e.g., near `end_line`), the top‑anchored slice may not be able to display a full `height` rows above it. In that case, clamp and allow the returned view to include rows below the anchor as needed. The priority is: include the anchor line; fill the rest within bounds; do not fabricate blank content.

### 4) Snapshot cadence & index accuracy

- If you snapshot at every delta today, the index will be dense and lookups trivial.
- If snapshot frequency is reduced later (e.g., every N deltas or every ~1s during activity), `snapshot_meta` still gives a quick starting point. Reconstructing from a snapshot requires applying only a small number of deltas.
- Ensure `add_snapshot` is called at the configured cadence and that `snapshot_meta` is always updated at the same point.

### 5) Keep client code unchanged (for now)

- The client already sends `ModifySubscription { mode: Historical, position: line: Some(...) }` when scrolling.
- With the server returning real historical frames, client view will stop showing blanks.
- Optional UX guard (future): keep rendering the last realtime frame until the first historical snapshot arrives; this is purely client‑side and does not require any logging or env vars.

## Files to Modify (Summary)

- `apps/beach/src/server/terminal_state/grid_history.rs`
  - Add `SnapshotMeta` struct and `snapshot_meta: Vec<SnapshotMeta>` field to `GridHistory`.
  - Update `add_snapshot(grid: Grid)` to compute and push `SnapshotMeta` (seq, start_line, end_line, timestamp).
  - Update `enforce_memory_limits()` (or wherever snapshots are evicted) to drop corresponding `snapshot_meta` entries.
  - Implement `pub fn get_from_line(&self, line: u64) -> Result<Grid, TerminalStateError>` as described.

- `apps/beach/src/server/terminal_state/grid_view.rs`
  - Update `pub fn derive_from_line(&self, line_num: u64, max_height: Option<u16>) -> Result<Grid, TerminalStateError>` to:
    - Call `history.get_from_line(line_num)`
    - Build a top‑anchored viewport of `max_height` rows from the reconstructed grid (if `max_height` is provided)
    - Preserve cursor correctness (hide cursor if above the new top)

## Separation of Concerns

- `GridHistory` is responsible for data retention and reconstruction from snapshots/deltas.
- `GridView` is responsible for exposing views (realtime/historical) by applying viewport logic to reconstructed grids.
- `TrackerDataSource` and `SubscriptionHub` remain transport‑agnostic consumers of `GridView`.
- The client remains a consumer of server views and does not own historical buffers.

## Validation Steps

1) Unit tests (server):
   - `GridHistory::get_from_line` with multiple snapshots and deltas:
     - Request lines well within a snapshot’s window
     - Request lines near snapshot boundaries
     - Request oldest/newest lines
   - `GridView::derive_from_line` returns the expected top‑anchored slice for a given `max_height`.

2) Manual run:
   - Start server and client; generate several screens of output (e.g., loop printing lines 1..200).
   - Scroll up in the client; verify non‑blank historical rows appear and match what was visible when those lines were printed.
   - Confirm realtime remains bottom‑aligned.

3) Performance sanity:
   - Observe that reconstructing from nearby snapshots is fast.
   - Verify memory stays within `HistoryConfig` limits.

## Non‑Goals / Out‑of‑Scope

- Adding new environment variables or printing to stdout/stderr.
- Client‑side scrollback buffering.
- Time‑based historical lookup (can be added later via `GridHistory::get_at_time`).

---

This plan keeps responsibilities clean and leverages your existing snapshot/delta machinery. Once implemented, scrolling up will display real historical content rather than blanks, without adding complexity to the client.

# beach-human tmux Equivalence Spec

## Scope and Goals
- Deliver a beach-human host + client experience that is indistinguishable from tmux in look, feel, hotkeys, and scrollback behaviour.
- Restore scrollback correctness with absolute row numbering and high-throughput history sync while keeping end-to-end latency competitive with native tmux.
- Align configuration knobs (`history-limit`, key tables, status line) so tmux users can swap between tmux and beach-human without relearning muscle memory.

## Current State and Gaps

### Terminal grid and emulator
- `TerminalGrid` stores rows in a `VecDeque` with a rolling `base` offset, but rows are addressed by their current index; the `_absolute` argument to `RowEntry::new` is thrown away (`apps/beach-human/src/cache/terminal/cache.rs:24`).
- The Alacritty-backed emulator uses `TermDimensions::new` with `total_lines = screen_lines`, so Alacritty never retains history and the server keeps overwriting the last viewport worth of rows (`apps/beach-human/src/server/terminal/emulator.rs:178-183`).
- Alacritty’s `viewport_top` is relative to the in-memory ring, so once scrollback is enabled we still need an explicit absolute row counter to prevent IDs from repeating after trims.

### Sync pipeline
- `TerminalSync` already exposes three `PriorityLane`s, but all lanes iterate over the same `grid.dims()` slice; there is no special handling for “initial snapshot vs deltas vs archived history” (`apps/beach-human/src/sync/terminal/sync.rs:101-194`).
- Trim events are emitted, yet the client has no back-pressure semantics for requesting older history or for pausing until the backfill arrives.

### Client viewport and controls
- `GridRenderer` tracks a `base_row` and scroll offsets, but it assumes that every row between `base_row` and `base_row + cells.len()` is resident (`apps/beach-human/src/client/grid_renderer.rs:90-186`). Missing rows result in blank gaps instead of tmux-like behaviour.
- Copy-mode bindings and movement are partially implemented and do not match tmux’s vi/emacs tables; there is no unified prefix handling or status line parity in the beach-human CLI (`apps/beach-human/src/client/terminal.rs:200-320`).

### tmux reference highlights
- tmux panes maintain `struct grid` with monotonic history via `hsize`/`hlimit` and drop 10% chunks when trimming (`tmp/tmux/grid.c:360-410`).
- `screen_alternate_on/off` clones the primary grid, disables history while in alternate screen, and restores it afterwards (`tmp/tmux/screen.c:629-703`).
- Copy-mode uses key tables and `grid_reader` helpers for vi/emacs navigation (`tmp/tmux/grid.c:3038-3056`).

## Proposed Phased Plan

### Phase 0 – Reference harness and parity checklist
**Objectives**
- Capture authoritative behaviour from tmux for scrollback, copy-mode, panes, and status line rendering.
- Establish automated diff tooling so every later phase can validate parity.

**Key work**
- Add `scripts/capture_tmux_transcript.sh` to spawn tmux, run a command script, and dump pane contents via `tmux capture-pane -e -p`.
- Extend `tests/client_transcripts.rs` with dual replay: beach-human transcript vs tmux reference; fail on any divergence.
- Define a tmux parity checklist document in `docs/` enumerating baseline behaviours (default key table, copy-mode navigation, status line prompts).

**Validation**
- CI gate: new transcript replay must show zero diff for at least the “scrollback 1000 lines” and “copy-mode navigation” scenarios.

### Phase 1 – Terminal grid + emulator scrollback revamp
**Goals**: Guarantee absolute row identifiers and retain configurable history (default 10_000 rows) without regressing write throughput.

**Server cache changes**
- Introduce `AbsoluteRowId(u64)` stored inside each `RowEntry` so the cache no longer depends on positional indices. Preserve `base` for fast trimming but expose helpers to translate id→index and index→id.
- Swap the `VecDeque<RowEntry>` for a ring buffer that tracks `(id, payloads, seqs)`; maintain a monotonic `next_row_id` counter on `TerminalGrid`.
- Make `DEFAULT_HISTORY_LIMIT` a runtime setting exposed via CLI/config; keep the default at 10_000 but allow overrides.
- When trimming, emit `TrimEvent { start_id, end_id }` so the sync layer can translate into tmux-style history drops.

**Emulator integration**
- Configure Alacritty with history by setting `TermDimensions::total_lines = screen_lines + history_limit` and, if necessary, calling `term.set_max_scrollback(history_limit)` once the API is available.
- Track absolute rows separately from Alacritty’s ring: maintain `next_row_id` inside `AlacrittyEmulator`, increment when `term.scroll_display(1)` or row damage indicates a wrap, and pass the absolute id into `CacheUpdate::Row/Cell`.
- Ensure alternate screen toggles keep the absolute counter advancing while history is frozen, mimicking `screen_alternate_on/off` semantics.

**Testing / benchmarking**
- Unit-test trim behaviour with sequences that exceed the history limit.
- Benchmark `write_packed_cell_if_newer` under high append rate to confirm the new id lookups do not introduce lock contention.

### Phase 2 – Multi-lane history sync pipeline
**Goals**: Stream the last N rows immediately, keep deltas hot, and drip-feed older history without blocking interactive use.

**New semantics**
- Add `INITIAL_SNAPSHOT_NUM_LINES` (default 500) to `SyncConfig` and expose it via CLI/env.
- Extend `ServerHello` to advertise `initial_snapshot_lines` and `history_limit` so the client knows when it can enable scrolling.

**Lane redesign**
- Replace the current `Foreground/Recent/History` iteration with explicit queues:
  1. **InitialSnapshot lane** – Grab the last `INITIAL_SNAPSHOT_NUM_LINES` rows (by absolute id) in descending order and send them as `CacheUpdate::Row` chunks until exhausted.
  2. **Delta lane** – Continue using `delta_stream` for live updates; treat trim notifications as priority messages.
  3. **Backfill lane** – Walk remaining history below `initial_snapshot_floor` in batches of `INITIAL_SNAPSHOT_NUM_LINES` rows; annotate batches with `[start_id, count]` so the client can request specific ranges when scrolling.
- Track per-subscriber cursors recording `highest_sent_row_id` and pending backfill ranges; store them in the existing `TerminalSnapshotCursor` or a new struct to avoid recomputation.
- Guard all lane queues by a lightweight mutex-free cursor (e.g., use `AtomicU64` for `next_history_id`) to keep the sync loop single-threaded but non-blocking.

**Protocol updates**
- Introduce a `WireHostFrame::HistoryBackfill { start_row, rows }` for low-priority batches so the client can differentiate them from authoritative deltas.
- Teach clients to emit `ClientMessage::RequestBackfill { start_row, rows }` when the user scrolls above the last cached id before a batch arrives.

**Observability**
- Instrument queue depths and time-to-delivery per lane (e.g., `sync.initial_snapshot.latency_ms`, `sync.backfill.pending_rows`).
- Add debug tooling to dump the server’s per-subscriber cursors for support scenarios.

### Phase 3 – Client viewport, scrollback, and copy-mode parity
**Goals**: The client should display history exactly like tmux, including copy-mode navigation, vi/emacs key tables, and placeholder behaviour while waiting for backfill.

**Viewport data model**
- Replace `Vec<Vec<CellState>>` with a sparse structure keyed by `AbsoluteRowId` (e.g., `BTreeMap<AbsoluteRowId, RowState>`). Track contiguous ranges so trimming and missing rows are explicit.
- Allow the renderer to mark rows as `Pending` until a `HistoryBackfill` chunk arrives; expose a “Loading…” overlay when the user scrolls faster than the server can backfill.
- Update selection and scroll helpers to operate on absolute ids instead of indices; adapt `base_row` / `scroll_top` to be offsets within the currently loaded range.

**Copy-mode and key tables**
- Mirror tmux’s vi/emacs key maps by defining key tables in TOML/JSON and loading them at runtime. Provide defaults that match tmux so `Ctrl-b [` enters copy-mode with identical navigation.
- Rewire predictive input to respect tmux’s `mode-keys` option (emacs by default) and update status messages to match tmux prompts.
- Ensure copy-mode search (`?`, `/`), jump (`f`, `F`, `t`, `T`), and selection yanks behave like tmux by reusing transcript fixtures from Phase 0.

**User feedback**
- Disable scrolling above the oldest cached row until the client receives confirmation that the backfill batch covering the requested id is in progress.
- Surface status line hints identical to tmux (e.g., “(end of scrollback)”) when the user reaches the limit.

### Phase 4 – Prefix handling, status line, and command UX
**Goals**: Align interactive controls so `Ctrl-b` driven workflows behave the same in both tmux and beach-human.

**Prefix + command mode**
- Implement a prefix state machine with configurable `prefix` and `prefix2` keys, honouring tmux’s `prefix-timeout` semantics.
- Add a tmux-style command prompt (`:`) with basic command parsing for window/pane operations (stubs initially) and help overlays.

**Status line**
- Render a status bar matching tmux defaults: session name, window list, activity flags. Allow customization via a subset of tmux format strings.
- Support status line messages (e.g., copy-mode hints, command prompts) using the same layout.

**Clipboard and paste buffers**
- Mirror tmux’s paste buffer stack and commands (`Ctrl-b ]`, `show-buffer`), integrating with system clipboard when allowed.

### Phase 5 – Windows, panes, and layout parity
**Goals**: Provide full tmux multiplexing semantics so users can split, resize, and navigate panes/windows identically.

**Pane model**
- Extend the server to manage multiple PTYs per session, each with its own `TerminalGrid` and history limit.
- Implement layout algorithms (even-horizontal, even-vertical, main-horizontal, main-vertical) mirroring tmux’s `layout.c`.
- Route input focus, resize events, and copy-mode context to the active pane with the same key bindings (`Ctrl-b o`, `Ctrl-b %`, `Ctrl-b "`).

**Window and session abstraction**
- Add lightweight session/window registries to beach-human so commands like `new-window`, `next-window`, and `rename-session` map to tmux’s behaviour.
- Model session attachment/detachment so multiple clients can share the same session while maintaining independent viewports.

**Transport implications**
- Extend protocol frames to include pane/window IDs and viewport metadata; ensure initial snapshot and backfill operate per-pane to avoid starvation.

### Phase 6 – Configuration compatibility and ecosystem polish
**Goals**: Reduce friction for tmux migrants by supporting familiar configuration flows and performance guarantees.

- Parse a subset of `.tmux.conf` (options related to key bindings, history, status line) and surface warnings for unsupported commands.
- Document migration steps and known gaps in `docs/tmux-parity.md`, highlighting intentionally unsupported tmux features (e.g., plugins requiring tmux’s command server).
- Build an automated benchmark harness that compares keystroke-to-render latency against tmux on the same host, failing if regressions exceed an agreed threshold.

## Performance and Testing Strategy
- **Benchmarks**: Extend the perf harness to stress-test history append (10k lines), backfill throughput, and copy-mode entry/exit latency. Track p99 enqueue→render timing per lane.
- **Unit tests**: Cover cache trimming, absolute id translation, snapshot batching, and copy-mode key tables.
- **Integration tests**: End-to-end transcript replays for long-running processes (`yes`, `ping`, `tail -f`) with asserts that the beach-human viewport exactly matches tmux.
- **Observability**: Expose Prometheus-style counters (`cache.rows_total`, `sync.initial_snapshot.delay_ms`) and structured log spans around lane processing and renderer ticks.

## Risks and Open Questions
- **Absolute row overflow**: Confirm `u64` is sufficient for long-lived sessions and decide when to recycle IDs (tmux resets on server restart).
- **Protocol churn**: Introducing new frame types requires backward compatibility or version negotiation with existing clients.
- **Multi-pane complexity**: Pane layout plus per-pane history dramatically increases state; consider shipping scrollback parity (Phases 1–3) before tackling panes to mitigate risk.
- **tmux option coverage**: Decide how far to go replicating tmux’s option matrix before declaring parity for v1.

Delivering these phases incrementally will restore scrollback fidelity first, then progressively layer tmux’s user-facing affordances until beach-human is a drop-in replacement for tmux.

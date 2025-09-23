# beach-human tmux Parity – Progress Snapshot

_Last updated: 2025-09-23_  
_Work-in-flight: Codex agent implementation pass_

## Completed Since This Pass Began
- **Absolute row IDs throughout server cache and emulator**
  - `TerminalGrid` rows carry `absolute` IDs, `next_row_id` monotonic counter, constant-time id↔index helpers.
  - Simple + Alacritty emulators respect history limit, seed absolute counters from cache.
- **Sync lane restructuring groundwork**
  - `SyncConfig` now exposes `initial_snapshot_lines`; serialized over the wire (`SyncConfigFrame`) and exercised in tests.
  - Snapshot cursors operate on absolute ids, Foreground/Recent lanes stream newest tail first, History lane goes idle until backfill work lands.
  - Handshake `SnapshotComplete` frames are emitted even when a lane produces zero chunks.
- **Client renderer migration to absolute rows**
  - Internal state uses `u64` row IDs, maintains `row_loaded` flags, and renders pending rows as dimmed `·` placeholders with status banner hint (“loading history”).
  - Predictions/selection logic updated to key on `(row: u64, col)`.
  - Copy-mode cursor and helpers now clamp via `i64`/`u64` aware helpers.
- **History backfill request pipeline**
  - Introduced `RequestBackfill` client frames and `HistoryBackfill` host responses, cap batches at 256 rows, and thread them through existing transports.
  - Terminal client now detects unloaded viewport spans, issues targeted requests, and marks fulfilled/missing rows to retire placeholders without manual refresh.
- **Transport heartbeat failover**
  - Heartbeat publisher demoted noisy warnings, applies exponential backoff, and activates a supervisor that renegotiates transports when the primary channel drops.
  - Shared transport wrapper lets running tasks swap in the refreshed connection without restarts.
- **Tests & tooling**
  - Updated sync/transport/unit tests for new config field and lane semantics.
  - Full `cargo test -p beach-human` passing.

## Outstanding Work
1. **History backfill scheduler polish**
   - Add fairness across participants, chunking/backpressure controls, and ensure style definitions hydrate alongside row data.
   - Thread cache dedupe through the new path so repeated requests avoid redundant payloads.
2. **Client sparse cache**
   - Current renderer still stores rows in a dense `Vec<Vec<CellState>>`; convert to sparse map or segmented ring for high history limits.
   - Track per-row watermark/seq to dedupe late deltas and ensure scroll blocking until data arrives.
3. **UX polish for pending history**
   - Disable scroll beyond oldest loaded row, surface progress (e.g., “loading rows 0–500”), allow manual retry or auto requests.
4. **Copy-mode / key tables parity**
   - Implement vi/emacs keymaps, tmux prefix handling, status-line prompts.
   - Integrate with new sparse cache so selection spans pending rows gracefully.
5. **Config surfacing & docs**
   - Expose `initial_snapshot_lines`, history limit, and placeholder behavior via CLI flags/config files; document tuning guidance.
6. **Backfill data pipeline tests**
   - Add transcript fixtures covering long scrollback, placeholder transitions, request/resume flows.
7. **Future phases**
   - Multi-pane/window layout parity, clipboard/paste buffers, tmux option file ingestion as outlined in `docs/tmux-equivalence-spec.md`.

## Immediate Follow-up Tasks
- Harden history backfill queueing (lane fairness, chunk sizing, retransmits, style hydration) and plumb through transmitter cache dedupe.
- Refactor renderer storage to sparse structure with load-tracking per row.
- Add integration tests asserting placeholder behavior and subsequent hydration when backfill arrives.

## Runbook Notes
- Server host command: `cargo run -p beach-human -- --session-server http://127.0.0.1:8080 ...`
- Client command: `cargo run -p beach-human -- --log-level trace --log-file ... join <session> --passcode ...`
- Current branch: `main` (uncommitted changes in cache, sync, client, protocol, tests).
- Tests: `cargo test -p beach-human`.

---

## Field Report: Placeholder Saturation & Backfill Gap
Client logs previously showed the viewport saturated with placeholder `·` rows even after new output streamed in. The new backfill request/response path changes that dynamic:
- Placeholders now trigger client-initiated `RequestBackfill` messages as soon as unloaded spans enter the viewport (with a configurable look-ahead window).
- Hosts answer with `HistoryBackfill` frames (up to 256 rows per batch), allowing the renderer to hydrate rows or mark them as trimmed so placeholders retire automatically.
- History beyond the server's retention window still resolves to blanks, but the UI no longer stalls waiting for unobtainable data.

**Remaining risks**
1. Scheduler fairness/backpressure is still naïve—single participants can monopolize the lane and large scrollback could require repeated requests after timeouts.
2. Style hydration piggybacks on existing cache state; older styles that never reappear may render with defaults until a dedicated style replay path lands.
3. Scroll safeguards are still loose; it is possible to scroll past hydrated content faster than the renderer can request successive batches.

Until the scheduler polish lands, extremely deep scrollback may require a second scroll pass if the first batch expires before it arrives, but placeholders no longer persist indefinitely.

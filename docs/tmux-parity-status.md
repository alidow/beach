# beach-human tmux Parity: Status & Next Steps

 _Last updated: 2025-09-24 (midday)_

## CRITICAL: Actual vs Desired Tail Behaviour
- **What we want**: After the handshake snapshot (a handful of rows), the host pushes every new line as a delta. The client applies those deltas in order, keeps the tail visible, and only issues history backfill requests when the user scrolls into an unloaded region.
- **What we do today**: As soon as the snapshot completes we aggressively request history (e.g. `start=33`, `start=154`) even while following the live tail. The host often responds with empty chunks, which our renderer treats as authoritative; it advances `base_row` past the missing span and leaves the freshly pushed rows wedged above a wall of `Pending` placeholders. We then keep re-requesting the same empty ranges and never show the full burst.

**Plan of Record (Sep 24 afternoon)**
1. Capture the misbehaviour in an integration test: simulate a minimal snapshot followed by a large burst of PTY output. Assert that the client stays on the pushed deltas (no redundant backfill requests) and renders all new rows.
2. Adjust the client pipeline so tail-follow mode relies on the streaming deltas, backfills only when the user scrolls into an unknown region, and never advances the base row on empty tail replies.
3. Re-run the live server/client burst once the test passes to confirm parity.


## Overview
Our original goal for this pass was straightforward: deliver a tmux-equivalent experience for beach-human so that the client behaves like a "dumb" view onto the host terminal buffer. The beach client should spin up quickly, mirror the host's scrollback accurately, stay responsive to input, and keep latency on par with tmux+ssh.

The recent work rebuilt a lot of infrastructure toward that goal (absolute row IDs, snapshot/backfill lanes, sparse client storage), but the most visible issues remain unresolved:

- Scrollback gaps still appear after large bursts of output—the client repaints the tail but leaves earlier rows blank.
- In the latest testing the client can get wedged in a `Pending` state (· placeholders) and stop responding to stdin entirely.

This doc captures what we have, what we tried, why it hasn't stuck, and what we should do next.

## Latest Update (Sep 23, late)
- Introduced a tail-first backfill path in `TerminalClient::maybe_request_backfill`. When the queue is clear we now immediately request the high range anchored to `highest_loaded_row`, so we stop collapsing toward `start=0` after large bursts.
- Pored over `~/beach-debug/host.log` and saw every tail backfill after `request_id=87` returning `delivered=0` while `more=true/false`, which keeps the UI stuck in `Pending` even though the host already told us the range was empty.
- Hardened `client_targets_tail_history_after_large_delta` so it drains handshake backfills and asserts the follow-up request targets the tail. The new assertions reproduce the earlier failure and pass with the scheduler fix.
- Added `client_marks_empty_backfill_as_missing` (`apps/beach-human/tests/client_transcripts.rs:878`) to capture the empty-tail wedge; it failed before the fix because the client immediately re-requested the same range.
- Empty backfill replies now mark rows `Missing` and keep `last_tail_backfill_start` sticky unless real rows arrive, so we stop spamming identical tail requests. Both targeted tests now pass.
- Reproduced the live "world vs world···for" mismatch with a unit test (`row_segment_overwrites_shrinks_row`) that feeds a short `RowSegment` after a longer command line. The client left the stale suffix, matching the UI screenshot. We updated `GridRenderer::apply_segment` to clear trailing cells when a segment rewrites from column zero, and the new test now passes.

## Field Notes (Sep 24)
Two regressions explain the "tail only" view we just reproduced while running the 150-line burst test.

- **Grid height advertised as total history length** – The host sends `HostFrame::Grid { rows, cols }` using `terminal_sync.grid().dims()` (`apps/beach-human/src/main.rs:1772`). `TerminalGrid::dims()` returns `(inner.len(), inner.cols())`, where `len()` counts *all* rows currently buffered, not just the visible window. When the grid has 200+ history rows the client seeds `GridRenderer::ensure_size(rows, cols)` with that large `rows` value, then `scroll_to_tail()` (`apps/beach-human/src/client/grid_renderer.rs:604`) immediately positions the viewport at `rows.len() - viewport_height`. The freshly streamed rows land well above the viewport, so we only see the newest tail slice until backfill catches up.  
  **Proposed fix**: teach the server to advertise the actual viewport height. `AlacrittyEmulator::render_full_internal` already knows `term_grid.screen_lines()`. Either (a) thread the emulator's `screen_lines` through `TerminalGrid` so `dims()` can return `(screen_lines, cols)`, or (b) override the rows field in the handshake with the PTY window height we already fetch during `process.resize(cols, rows)` (`apps/beach-human/src/main.rs:1087`). Clients should keep tracking absolute rows, but the initial `rows` needs to be "visible rows" so the viewport math stays sane.

- **Local banners never reach the replicated grid** – The host prints `print_host_banner(...)` (`apps/beach-human/src/main.rs:557`) *before* the PTY is live, so "🏖️ beach session ready!" and the share command never enter the grid cache. The joiner prints its own "🌊 Joined session" banner (`apps/beach-human/src/main.rs:574`) locally, also outside the synchronized stream. When we compare host/guest transcripts the row counts are offset by those local-only banners, which makes it appear as if the client missed early lines.  
  **Proposed fix**: either move the banner printing to the PTY (e.g. write through `PtyWriter` right after spawn) or flag the banners in the UI so users know they are out-of-band. If we keep them local, the client should subtract their height from any "current row" calculations or we should surface a status message clarifying that replication begins after the host prompt.

Taken together, the bogus `rows` value explains why the client only displayed lines 112–150 after the burst, while the missing banners explain why the host transcript always seems "longer" than what the client renders. Fixing the handshake dimensions should restore parity for live output; redirecting the banners into the PTY (or hiding them) will eliminate the row-number mismatch during diffing.

### Test Harness Sketch
Before we touch the implementation, we can lock the behaviour in a unit-style integration test by mocking the PTY and transport stack:

1. Use `TransportPair` from `tests/transport_sync.rs` to link a host/server stack with an in-process "client" transport so we can capture `HostFrame::Grid` and subsequent updates without spinning up the CLI.
2. Build a `MockTerminalSetup` that exposes:
   - a `TerminalGrid` pre-populated with 240 absolute rows of history;
   - a shim `TerminalEmulator` whose `TermDimensions` reports `screen_lines = 24` and `total_lines = 240`, matching what the real Alacritty backend would see when history is present.
3. Inject this mock into `initialize_transport_snapshot` (mirroring the pattern in `tests/session_roundtrip.rs`): kick the handshake, then assert that the first emitted `HostFrame::Grid` advertises `rows == 24` (the viewport) instead of `240` (the scrollback length). This reproduces today's failure without touching the real PTY.
4. In the same harness, feed a fake PTY write (`write(&[b'T'; 150])`) through the mocked emulator and confirm the client-side renderer receives the first line immediately once the handshake rows are correct.

This keeps the regression self-contained: no subprocess spawn, just mocked PTY dimensions and the existing transport pair. Once the fix is in place the test will fail if someone accidentally reverts to using `grid().len()` for the handshake rows.

**Status (implemented Sep 24):** The host now tracks `viewport_size` on `TerminalGrid`, `initialize_transport_snapshot` emits the PTY height/width instead of the scrollback span, and `spawn_input_listener` updates the viewport on every resize. The new `handshake_advertises_viewport_height_even_with_history` test drives the mocked transport pair and fails if the handshake ever regresses to the old behaviour.

Follow-up fix: `GridRenderer::scroll_to_tail` now walks back from the last *loaded* row before positioning the viewport, so empty tail backfills can no longer leave the UI parked on a wall of `·` placeholders. The new `follow_tail_prefers_loaded_rows_after_empty_tail_backfill` test captures the regression.

## New Live Failure (Sep 24, 17:04)
- Repro: session `4fde21ec-3a4e-46cb-b3a6-ebef566fde4d`, host ran `echo hi` followed by the same `for i in {1..150}` burst. The guest never rendered any text (all slots stayed `Pending`) and the status banner remained `loading hist`.
- Host log shows the expected sequence: initial `RequestBackfill start=0 count=256` returned 24 populated rows, then three empty chunks (`delivered=0`). Immediately afterwards the host streamed deltas for rows `112..150` with watermarks climbing past 23 k.
- Client log confirms `WireHostFrame::Delta` frames arrived, but `highest_loaded_row` never advanced past 23 and `known_base_row` jumped forward to 258. Every subsequent `RequestBackfill` from the client started at 258 and requested 142 rows, so we completely skipped the gap we actually needed (rows 24–111).
- `GridRenderer::first_gap_between` now reports `(start=258 span=142)` once the tail burst finishes, meaning the renderer believes everything below 258 is already hydrated. That matches the symptom: the delta rows were applied, but because `set_base_row` had been raised by the empty history replies we immediately trimmed the lower portion away.
- The new tests (`client_requests_backfill_and_hydrates_rows`, `client_resolves_missing_rows_after_empty_backfill`, `client_recovers_truncated_history_after_tail_burst`) illustrate the same issue: without grounding the base row, empty replies advance the cursor and the scheduler jumps straight to the tail.

### Diagnosis
1. Empty history replies still allow `finalize_backfill_range` to call `set_base_row(bounds_start)`. With `bounds_start` derived from the (empty) span it ends up at `start_row + count`, effectively trimming the history we still need.
2. Once the base row moves forward the subsequent delta rows land **ahead** of the viewport. `note_loaded_row` is called with absolute row IDs, but because the renderer dropped everything below 258 we never mark rows 112–150 as loaded.
3. The gap scheduler we just added is guarded by `highest_loaded_row - known_base_row > BACKFILL_LOOKAHEAD_ROWS`. When `highest_loaded_row` stays at 23 that branch never fires and we only try to backfill the tail.
4. The live CLI therefore shows only placeholders: we believe the missing rows are a “gap” above 258, so we keep asking for that range and the host keeps replying with nothing.

### What We Tried Today
- Added `GridRenderer::first_gap_between` and taught `maybe_request_backfill` to probe the lowest unloaded span before chasing the tail. Works in unit tests, but the live trace shows `first_gap_between` already returning 258 because the base row was moved up by the empty reply.
- Updated `client_marks_empty_backfill_as_missing` to expect a retry and added `client_recovers_truncated_history_after_tail_burst` to lock down the gap-filling behaviour. Both pass in isolation, but only because the synthetic scenario keeps the base row anchored.
- Introduced `RecordingTransport` tests to confirm the bridge request is issued when the gap is below the tail. They still rely on the base row staying put, so they pass even though the live system trips earlier in the pipeline.

### Next Steps
1. **Anchor `base_row` on empty history** – When `finalize_backfill_range` receives an empty reply, leave the base row and `known_base_row` untouched and mark the span `Pending` to trigger retries. Only move the base row when we actually populate rows.
2. **Force a bridge when deltas land below the current base** – Detect deltas for rows `< known_base_row` and immediately queue a backfill starting at that absolute row, regardless of `highest_loaded_row`. This guarantees we revisit the truncated span even after empty replies.
3. **Instrument `note_loaded_row`** – Add one-off tracing (or expose a test-only hook) to confirm we’re recording the true highest row when deltas arrive during recovery. Without that visibility we can’t tell if the issue is trimming or a broken call path.
4. **Handshake fix** – Prioritise sending the PTY viewport height in the initial `Grid` frame. Even once the gap logic works, the incorrect row count keeps the viewport anchored far below the interesting rows.
5. **Rerun the live session** with the diagnostic logging, verify the client requests the 24–111 span, and confirm the UI finally renders lines 1..150 before we make any more structural changes.

## Current Progress Snapshot
- ✅ **Absolute row IDs everywhere** – Host grid/emulator and both delta/snapshot paths now use absolute row numbers. This removed the ID drift that used to poison history.
- ✅ **Sync lane restructuring & throttled backfill** – We introduced foreground/recent/history lanes with chunking and throttling, so history flows through a queue instead of blocking deltas.
- ✅ **Sparse renderer on the client** – The TUI now tracks `Pending`/`Loaded`/`Missing` slots with per-row watermarks, keeps placeholders while history loads, and handles trims.
- ✅ **Backfill pipeline end-to-end** – `RequestBackfill` frames travel over transports, the host responds in slices, and we have integration tests for populated + empty responses.
- ⚠️ **Tail parity** – The tail scheduler now re-requests the high range (`highest_loaded_row - lookahead`) once the queue clears, and empty replies downgrade rows to `Missing`. Live parity still unverified, so we need a fresh repro run before we call this solved.
- ❌ **Responsiveness** – In the most recent run the client stalled entirely: placeholders never resolved (`loading hist` banner) and stdin was ignored until we killed the session.

## What We Tried (and what broke)
1. **Handshake refresh fix** – We disabled the handshake watchdog after completion so it stopped blowing away pending rows. This worked, but gaps persisted once we scrolled past the initial baseline.
2. **Backfill dedupe tweaks** – We switched history responses to skip dedupe so clients receive rows even if the cache already saw them. Host logs confirmed the rows were sent, but the client was still blank because we never targeted the right absolute IDs.
3. **Known base row / highest row tracking** – We added `known_base_row` and `highest_loaded_row` to keep backfill requests aligned with the data we actually have. The logic still clamps to the first unloaded span (near 24), so we re-request the low range.
4. **Empty history handling** – We marked empty replies as `Pending` to force retries, then as `Missing` to unblock the renderer. That removed some flicker but also caused wide retry storms when the server legitimately had nothing to send.
5. **Integration tests** – We added:
   - `client_retries_history_when_initial_backfill_empty`
   - `client_targets_tail_history_after_large_delta`
   - A renderer-level check that the tail isn't blank after new rows arrive

   These now catch the regression where we collapsed back to `start=0`; the new empty-tail test covers the wedge we just reproduced.
6. **Tail-first scheduler** – Added `last_tail_backfill_start` tracking plus a short-circuit in `maybe_request_backfill` so we prime the tail before re-scanning low ranges. The fix passes the updated integration test but hasn't seen a live tmux parity run yet.
7. **Empty reply semantics** – `finalize_backfill_range` now marks untouched rows as `Missing` and we only clear `last_tail_backfill_start` when we actually receive data. This stops the retry storm and lets the UI unblock even when history is genuinely blank.
8. **RowSegment cleanup** – `GridRenderer::apply_segment` now scrubs trailing cells when a segment starts at column zero, preventing stale prompt text (`…for`) from bleeding into shorter outputs.

## Latest Failure (Sept 23, 15:58)
- Host sends deltas up to watermark ≈ 12207 right away.
- Client issues backfills: `start=24 count=256`, `start=78 count=178`, ...
- Every reply logs `updates=0`, so the renderer stays `Pending` and our next request keeps marching backward (`start=0`).
- UI shows dots, `loading hist`, stdin appears unresponsive because the event loop is waiting on history to settle.

This transcript is from the pre-fix run; repeat it once we validate the new scheduler against the real host.

## Proposed Next Steps
1. **Replay the high-burst repro** – Run the tmux parity scenario that previously wedged the client to confirm the scheduler fix keeps the tail populated and input responsive. Capture logs to verify the new tail request range and confirm placeholders disappear.
2. **Audit host scrollback after restore** – Investigate why the host only had data for the first 24 rows (`request_id=87` chunk) and nothing for the tail. We may be restoring a truncated buffer, which would explain the blank output even though the client now recovers.
3. **Input loop resilience** – Audit the client event loop: when history is pending, timers or queue processing shouldn't starve input. We may need a watchdog to ensure we render even when history is empty.
4. **Telemetry / logs** – Keep the `client::render` trace for now; add aggregated metrics (`tail_backfill_start`, `pending_rows`) so we can confirm the fix without poring over raw logs.
5. **Regression coverage for restores** – Extend the transcript harness with a "restored session" fixture so we can assert both the host snapshot and the client's empty-tail handling before we ship.

Once the live repro stays green and the empty-tail test covers the remaining gap, we can finally focus on the parity polish items (copy-mode keys, status line, etc.).

---

**TL;DR** – Tail scheduling plus the empty-backfill fix stop the client from wedging in `Pending`, and `row_segment_overwrites_shrinks_row` proves we finally clear the stale `…for` suffix when short outputs land. Next: re-run the live tmux repro, verify the tail stays populated, and figure out why restores still start with such a short history window.

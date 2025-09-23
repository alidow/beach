# beach-human tmux Parity: Status & Next Steps

_Last updated: 2025-09-23 (near midnight)_

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

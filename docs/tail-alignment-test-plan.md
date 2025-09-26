# Tail Alignment Integration Test Plan

## Current Behaviour
- Reproducing sessions still show the tail anchored at `base_row=0` with dot placeholders.
- Client attempts multiple backfills (`start_row=89 count=22`), even after receiving trim updates.
- `client::tail` traces confirm base row oscillates back to `0` after empty backfill replies.

## Existing Work
1. Added `history_backfill_trim_regression_repro` unit-style test in `client/terminal.rs`:
   - Replays handshake + trim-bearing history backfill.
   - Previously asserted trim regressions; now asserts base remains ≥ trimmed origin.
   - Test currently passes (reproduces new logic but not full end-to-end scenario).
2. Added TRACE hooks in `GridRenderer::set_base_row` and `apply_trim` to expose base changes.
   - Confirmed runner receives trim then reverts base on empty backfill.

## Integration Test Attempt (WIP)
- Goal: End-to-end test using `TransportPair` to simulate server-client handshake with trimmed history.
- Scenario steps identified (server messages, expected client states).
- Partial implementation inserted in `client/terminal.rs::tail_alignment_end_to_end_regression`.
- Issues encountered:
  * Backfill requests must be read from `TerminalClient::pending_backfills`; no direct server recv helper.
  * Test currently expects second (empty) backfill to regress base (i.e., bug present).
  * Implementation still relies on direct `maybe_request_backfill` calls and manual frame delivery.

## Next Steps
1. Finalize integration test logic:
   - Use direct `pending_backfills` inspection for request IDs/start rows.
   - Deliver second empty backfill and assert tail remains at corrected base (fail until bug fixed).
   - Replace manual transport loops with helper functions ensuring minimal flakiness.
2. Restore original trim logic once the regression test is failing-for-real.
3. Implement fix to prevent base regression (likely in `finalize_backfill_range` logic) and update assertions.
4. Run `cargo test -p beach-human tail_alignment_end_to_end_regression` to validate.
5. Optionally add server-side portion (mocked PTY grid) for comprehensive coverage.

## Notes for Next Engineer
- Logs to inspect: `client::render event="apply_trim"`, `event="set_base_row"` and `client::backfill` traces.
- Pay attention to `known_base_row` updates; ensure trim bumps this value and empty backfills ignore older rows.
- Review `finalize_backfill_range` and `observe_update_bounds` interactions.
- Integration test should fail before fix (base regress < 400), pass after.


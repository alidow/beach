# Tail Alignment Refactor Notes

Goal: keep the client’s tail anchored correctly after history trim and eliminate the dotted gap. Current gaps traced back to the host advertising absolute row IDs from an old PTY session without telling the client what the base row actually is.

## Outstanding work
- Extend `HostFrame::Grid` with a `base_row` and propagate it through encode/decode. Server must send `terminal_sync.grid().row_offset()`.
- Client should sync `GridRenderer` with that base as soon as the handshake lands. Update `GridRenderer` to remember whether history has been trimmed (`history_trimmed = base_row > 0`) and include that flag in its tail padding logic.
- Finish updating tests: every fixture emitting `HostFrame::Grid` needs to specify `base_row`. Unit tests already cover both top-aligned (base 0) and trimmed buffers.
- Emulator currently calls `ensure_session_origin` and also sets the grid row offset. Make sure we do not double-set or regress snapshots; the helper now just returns the origin and the caller calls `grid.set_row_offset(origin)`.

## Diagnostics already in place
We added `client::tail` TRACE logs inside `visible_lines` / `render_body` that dump base row, scroll top, viewport, a small sample of rows (`P` pending vs `L` loaded), and the last rendered line. Run the tmux repro and check those logs; if base row stays at 0 after the for-loop, the handshake didn’t apply the new base.

## Next steps
1. Finish wiring `base_row` through `protocol::HostFrame`, `wire.rs`, client handlers, and tests.
2. Confirm `GridRenderer::set_base_row()` and the new `set_history_origin()` are called from the handshake and from history trims.
3. Verify via repro: server + client run, tail rows should sit on the bottom; log should show `client::tail` last line `Line 150` with `base_row ~ 128`.
4. Once behavior is correct, tone down TRACE or remove extra logging before landing.

This is the state the branch is in; another Codex instance can pick up from here.

# Normalising Terminal Row Numbers

## Problem
- The server currently emits row indices taken directly from the PTY/terminal emulator.
- macOS Terminal, zsh plugins, login banners, build output, etc. can emit dozens or hundreds of lines before the user command runs.
- Those pre-existing lines cause our absolute row counter to start at an arbitrary offset (e.g. row 118).
- The client interprets missing lower indices as gaps and renders `Pending` placeholders even though the PTY never rewrote those rows during this session.
- Asking every user to reset scrollback or tweak their shell is not viable; the fix must work anywhere (any shell, OS, or command).

## Goal
Guarantee row `0` always maps to the first line the user sees after the host session starts, independent of whatever history the PTY previously contained.

## Proposed Fix (Clean + Universal)
1. **Capture a session origin at attach time.**
   - When the emulator renders the initial snapshot, compute the absolute row ID of the top visible line using the PTY metadata (`base_row + viewport_top`).
   - Store this as `session_origin` in the emulator.
2. **Rebase every outgoing update.**
   - For all `CacheUpdate::Cell`, `CacheUpdate::Row`, and `CacheUpdate::Rect` updates, subtract `session_origin` before queuing them.
   - Clamp at zero so the first visible line is always row `0`.
3. **Handle PTY hard resets.**
   - If the PTY clears scrollback (e.g. `ESC c`) and the absolute top becomes `< session_origin`, emit a `CacheUpdate::Trim` to drop prior rows, reset `session_origin`, and continue rebasing from the new origin.

## Why This Works Everywhere
- We already spawn a fresh PTY; the remaining variability is how much text the child program prints. Rebasing makes that immaterial.
- Nothing in the client or transport needs to change: all consumers keep receiving monotonically increasing row IDs that start at zero per session.
- No dependency on shell configuration, terminal emulator features, or user discipline.

## Implementation Notes
- Store `session_origin` inside both emulators (`AlacrittyEmulator`, `SimpleTerminalEmulator`), initialised from the first snapshot.
- Update `render_full_internal` and `collect_updates` to use `relative_row = absolute_row.saturating_sub(session_origin)`.
- Ensure `TerminalGrid::write_*` continues to accept zero-based rows; existing tests should continue to pass once inputs are rebased.
- Add a regression test that feeds in a synthetic absolute history (e.g. starting at 118) and verifies the emitted updates begin at row `0`.

## Validation
- Unit test: simulate prefilled history + new output; assert row IDs are rebased to `0..N`.
- Manual: run the macOS Terminal repro without clearing scrollback; server logs should now show `server::grid row=0..` for the burst, and the client should render a contiguous tail with no placeholders.

This approach keeps the protocol simple, avoids per-shell hacks, and guarantees consistent behaviour on any platform.

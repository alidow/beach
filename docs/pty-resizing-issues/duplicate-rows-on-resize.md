# PTY Resizing & Viewer Duplication

## Summary

- When a Private Beach tile is resized taller than the underlying PTY, `BeachTerminal` continues to render the old rows to fill the extra space, producing the illusion of duplicated HUD/output.
- The harness already accepts `resize` frames from the viewer and resizes the PTY (`apps/beach/src/server/terminal/host.rs`).
- Our Pong TUI now clears the expanded region each frame, so the server-side buffer is clean, yet the viewer still mirrors rows to fill space.

## How It Happens

1. `ResizeObserver` in `BeachTerminal` measures the new viewport height and sends `{ type: 'resize', cols, rows }` to the harness.
2. The harness clamps and applies the resize, resizes the PTY, updates the emulator, and pushes diffs downstream.
3. The viewer asks the terminal cache (`apps/beach-surfer/src/terminal/cache.ts`) for `height` rows. If the requested height exceeds the buffered content, it keeps walking up the history, reusing existing rows. It doesn’t pad with blanks.
4. The UI fills the entire tile with those “extra” rows, effectively repeating earlier lines (HUD, prior output, etc.) higher in the viewport.

## Why It’s a Bug

- The PTY is already larger—our app has updated geometry and redraws once per frame.
- The viewer should render blank space above, not duplicate the same rows. Reusing rows breaks visual expectations (appears out of sync, looks like double rendering).

## Fix Proposal

**On the Viewer (`BeachTerminal`)**

1. Clamp the number of rendered rows to the PTY’s viewport (`snapshot.viewportHeight`) to avoid exceeding the actual buffer.
2. Alternatively, pad with “missing” rows instead of replaying historical content. That leaves empty space, matching the PTY’s state.
3. Optionally auto-trigger the “Match PTY size” action when a tile grows—only if the session is in controller mode—to keep PTY and viewer in sync automatically.

**On Host Apps (already implemented)**

- Clear any rows near the bottom that relocate after a resize (ensures no stale text stays in the PTY). Pong now tracks HUD rows and clears them each frame.

## Next Steps

1. Update `BeachTerminal.visibleRows()` logic to pad or clamp, preventing visual duplication (`apps/beach-surfer/src/terminal/cache.ts`).
2. Decide on controller locking: only the session holding the control lease should drive PTY resize.
3. Add regression tests in `BeachTerminal.lines.test.ts` ensuring the viewer doesn’t re-render missing rows with existing content.

Once the viewer pads correctly (or clamps to the PTY height), stretched tiles will show blank space instead of duplicating the HUD, and the Pong TUI will stay clean regardless of viewport size.

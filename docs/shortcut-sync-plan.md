# Shortcut-Driven Clear Handling Plan

## Problem Statement

Beach currently assumes that every key binding directly reaches the
server-side PTY. On macOS, Terminal.app intercepts `⌘K` (and a handful of other
“UI” shortcuts) before user-space applications see them, so the host PTY never
receives the clear-screen request. The Rust client clears its own view, but the
canonical `TerminalGrid` on the server is untouched, leading to persistent
desyncs between host and viewers.

The goal is to guarantee that every viewport mutation is mediated through the
shared cache so all clients render identical state, regardless of local terminal
behaviour.

## Observations

- Terminal.app binds `⌘K` to *Clear to Start* at the OS level. Neither stdin nor
  low-level libraries (crossterm, termios) receive the key event. iTerm2 exposes
  custom keypad mappings, but the default profile still eats `⌘K` unless the
  user rebinds it.
- tmux does not rely on the terminal for these shortcuts. The default config
  explicitly maps `C-k` (or other keys) to `send-keys C-l \; clear-history`, so
  tmux itself injects the control sequence and prunes its history buffers.
- Other “UI” chords (e.g. `⌘T`, `⌘W`, `⌘N`, Mission Control shortcuts) are also
  swallowed by Terminal.app/iTerm2. We cannot rely on raw key codes for any
  platform-level shortcut.

## Long-Term Plan

### 1. Explicit control frames

Introduce a new client-to-host wire frame (e.g. `ClientFrame::ViewportCommand`)
with an enum payload (`ClearViewport`, `ResetScrollback`, etc.). The Rust client
should emit this frame when it detects a shortcut that could be intercepted by
the terminal UI.

- For macOS, detect `⌘K` via crossterm (it reports modifiers even when the
  terminal consumes the keypress entirely). When the event arrives, **do not**
  clear the local renderer impulsively. Instead:
  1. Send `ViewportCommand::ClearViewport`.
  2. Queue the canonical `Ctrl-L` bytes into the existing input path (to mimic
     tmux’s `send-keys C-l`).
  3. Suppress predictive rendering for this injection so we do not fabricate
     rows that the server never confirmed.
- Allow for future commands (copy-mode shortcuts, splits) without changing the
  transport contract again.

### 2. Server-side handling

Extend the host loop to recognise the new frame:

1. Emit the same `Ctrl-L` bytes to the PTY (`PtyWriter::write_str("\u{000C}")`).
2. After the PTY finishes flushing, trim the authoritative `TerminalGrid`:
   - Reuse the logic tmux’s `cmd_clear_history_entry` applies to force the view
     to row zero while discarding scrollback rows.
   - Update `grid.row_offset()` and reset `history_limit` counters so deltas
     start from a known state.
3. Broadcast a fresh `HostFrame::Snapshot` (or `HostFrame::Grid` + initial
   snapshot) so every client rehydrates from the trimmed cache.

This guarantees that the grid cache and all clients reflect the same “cleared”
viewport. The server remains the single source of truth; clients act as dumb
renderers.

### 3. Client renderer adjustments

The client should wait for the authoritative updates and only redraw once they
arrive. Two small improvements keep the UX smooth:

- Force a `render()` tick immediately after sending the command so the local
  user sees the screen wipe without waiting for the next transport frame.
- When applying `CacheUpdate::Row`/`Rect` clears, flush any outstanding
  predictions or copy-mode highlights across the affected rows (already handled
  by the recent patches).

### 4. Detecting swallowed shortcuts comprehensively

Because terminal emulators handle many OS shortcuts, we need a strategy that
doesn’t rely on catching *every* key combination:

- Maintain a small registry of “critical shortcuts” we emulate ourselves
  (`⌘K`, `⌘C`, `⌘V`, copy-mode binds). The registry can be platform-specific and
  user-configurable.
- Allow users to opt into different bindings (e.g. map `Ctrl-L` directly) via
  the client config so Linux/BSD users aren’t forced into macOS-style behaviour.
- Provide a diagnostic mode (`beach --debug-keys`) that prints raw events. If a
  shortcut yields nothing, document that the terminal emulator intercepted it
  and recommend rebinding (or using our explicit command palette).

### 5. Tests & validation

- Integration test: spin up host + client, inject `ViewportCommand::Clear` via
  transport mocks, assert that the server `TerminalGrid` has zero visible rows
  and that the client renderer matches.
- End-to-end script for macOS CI: drive the real binary with `script` or `expect`
  to verify the server logs the clear frame and no stale rows remain.
- Regression test ensuring the tmux-like status bar persists post-clear.

## Open Questions & Follow-ups

- **Terminal detection:** we cannot programmatically detect `⌘K` if the emulator
  swallows the event before crossterm sees it. Our mitigation is to expose the
  binding at the beach layer (e.g. bind `Ctrl-Shift-K`) and encourage
  users to rebind Terminal/iTerm2 to forward the shortcut. Document this in the
  CLI help and onboarding guide.
- **Other shortcuts:** Audit the default terminal bindings (`⌘W`, `⌘T`, Mission
  Control) and provide fallbacks or warnings. For commands that cannot be
  remapped safely, offer a command palette entry (`:clear`, `:split`, etc.).
- **Config sync:** Consider shipping default configs per platform to mirror
  tmux’s terminal-specific adjustments.

## Next Steps

1. Define `ViewportCommand` in the wire protocol and plumb it through the client
   and host loops.
2. Implement PTY injection + grid trimming on the host.
3. Update the renderer to rely on authoritative updates post-clear.
4. Document the new shortcut behaviour and surface platform-specific guidance in
   `docs/user-guide.md`.
5. Add integration tests covering the new flow.

This roadmap ensures the caches remain canonical and every client, regardless
of local terminal quirks, renders the same state.

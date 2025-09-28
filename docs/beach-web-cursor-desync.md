# Beach Web Cursor Desync Investigation

## Summary

The Beach web terminal now renders a highlighted cell to emulate the host
cursor, but the visual caret is still misaligned with the true cursor position.
Two issues remain:

- On the interactive shell prompt the highlight lands one column too far to the
  left (e.g. the block appears on `%` instead of the trailing blank after it).
- When the user clears the line (for example with `Ctrl+U`) the terminal on the
  host side rewinds to column 0, yet the web cursor highlight stays at the
  previous column until new printable characters arrive.

These symptoms indicate the client is not interpreting the cursor column that
originates from the host correctly and does not track cursor-only motion (row
clears, carriage returns, etc.) unless a text update also lands on that cell.

## Current Implementation

### Cache level

- `apps/beach-web/src/terminal/cache.ts:181` stores cursor hints derived from
  each update. The cache eventually surfaces `cursorRow`/`cursorCol` in the
  snapshot used by React.
- Hints are inferred heuristically:
  - `cell` updates treat `col + 1` as the cursor column after the write.
  - `row_segment` uses `startCol + cells.length` (or `rowDisplayWidth` if the
    segment is empty).
  - `rect` updates assume the cursor ends at the right edge (`col_range[1]`).
- `rowDisplayWidth` trims trailing blanks (`apps/beach-web/src/terminal/cache.ts:398`)
  before returning the width.

### Renderer level

- `buildLines` forwards the raw `cursorCol` with no additional clamping
  (`apps/beach-web/src/components/BeachTerminal.tsx:327`).
- `LineRow` highlights the indexed cell and appends an extra blank `<span>` when
  `cursorCol >= cells.length` (`apps/beach-web/src/components/BeachTerminal.tsx:360`).

### Host behaviour (Rust client)

- The host-side `GridRenderer` keeps its own `cursor_row`/`cursor_col` that are
  updated by `cursor_hint`s inside `apply_wire_update`
  (`apps/beach-human/src/client/terminal.rs:1101`).
- Cursor hints are derived from the same wire updates that the web cache consumes:
  - `Cell` -> `(row, col + 1)`.
  - `Row`  -> `(row, row_width)`.
  - `RowSegment` -> trailing column of the segment.
  - `Rect` -> `(rows[1] - 1, cols[1])`.
- Because the host follows local cursor movement perfectly, we know the raw
  updates contain enough information; the discrepancy is introduced inside the
  web client.

## Observations

1. **Prompt misalignment**
   - After connecting to a live session the prompt renders correctly, but the
     highlighted span sits on the final glyph (`%`) rather than the blank cell
     after it.
   - Inspecting `window.beachStore.getSnapshot().cursorCol` shows a value equal
     to the prompt length (e.g. `48`, matching `rowDisplayWidth`). The web
     renderer highlights index `cursorCol` directly, which selects the `%`
     cell (0-based index). The host, however, expects the cursor to sit on the
     following blank cell (i.e. 1-based indexing or “next” column semantics).
   - The store currently adds `+1` for `cell` and `row_segment` hints but **not
     for `row` and `rect` hints**. When the history is back-filled via `row`
     updates the cursor lands on `row_width`, which is already trimmed – no extra
     column is added. By the time the prompt arrives the cache has combined
     multiple hint sources, producing an off-by-one column.

2. **Ctrl+U / line clear**
   - The host emits `ESC [K` (clear to end of line), which the emulator converts
     to a `Rect` update spanning the current column to the viewport width. The
     cursor should stay at the original column or, after Ctrl+U, move to column
     0 (preceded by a carriage return).
   - The cache converts `rect` to a `row_width` hint (`apps/beach-web/src/terminal/cache.ts:360`),
     but `rowDisplayWidth` considers only printable data. After the line is
     cleared the row width becomes `0`, therefore the cache sets `cursorCol = 0`.
     In practice the highlighted span remains at the previous column, which means
     **the cache never applied that hint**. Logging shows `cursorHint(rect)` is
     evaluated *after* `applyRect`, and `applyRect` returns `false` when the fill
     does not modify a cell (e.g. the row was already blank). Consequently the
     outer `mutated` flag stays `false` and the snapshot is not invalidated, so
     React keeps rendering the stale cursor column.

## Reproduction steps

1. Start the host session: `cargo run -- --session-server http://127.0.0.1:8080`.
2. Open the web client (`pnpm dev` inside `apps/beach-web`).
3. Join the session and wait for the `%` prompt.
4. Observe that the cursor highlight appears on the `%` glyph.
5. Type a command, then press `Ctrl+U`. The shell prompt rewinds to column 0
   while the highlight remains where the command previously ended.

## Hypothesis / Root cause

The cache’s cursor inference assumes a mixture of 0-based and 1-based columns:

- `Cell` and non-empty `RowSegment` hints apply `col + 1`, effectively treating
  `col` as the index of the character that was written.
- `Row` hints use `rowDisplayWidth`, yielding the count of printable columns.
- When history is bootstrapped via `Row` updates the cursor column becomes equal
  to the last printable column, i.e. **one less than the “next position”**. The
  renderer therefore highlights the wrong cell.
- For cursor-only moves (line clear, carriage return) the cache often receives a
  `Rect` update that fills blanks. When the fill does not mutate any cells,
  `applyRect` returns `false`, the outer loop never marks the cache as mutated,
  and the newly computed cursor hint is not persisted. The cursor column stays
  stale until another update touches the row.

## Suggested next steps

1. **Normalize cursor semantics**: decide on a single coordinate system (e.g.
   “cursorCol always points to the next write position”) and update every hint
   path accordingly.
   - Likely change: `cursorHint('row')` should return `{ exact, col: row_width }`
     **and add +1** to match the cell/segment behaviour, or we should remove the
     `+1` from all other paths and clamp in the renderer instead.
2. **Persist cursor-only hints even without grid mutations**:
   - Update `applyUpdates` to treat cursor hints as mutations on their own or
     record `cursorDirty` separately so snapshots invalidate whenever the cursor
     changes (`apps/beach-web/src/terminal/cache.ts:181`).
   - Alternatively, make `applyRect` report a mutation whenever it processes a
     different `seq`, even if the visual content stays the same.
3. **Add instrumentation**:
   - Temporary logging in the cache to trace `(update.type, cursorHint, cursorCol)`.
   - Capture a short session log (host + devtools console) demonstrating the
     incorrect cursor column for inclusion in automated tests later.
4. **Regression tests**:
   - Unit-test the cache against synthetic sequences (prompt render, carriage
     return, line clear) once the semantics are fixed.

## Open questions

- Does the host ever emit an explicit cursor position frame? If so, it might be
  preferable to consume it directly instead of inferring from updates.
- Should the web client surface cursor visibility/blink state in addition to the
  position (currently we always render a solid reverse-video block)?
- The existing renderer assumes monospace layout; if proportional fonts are ever
  allowed the cursor logic will need a different representation.

## Contacts / History

- Recent changes: virtualized scrolling and initial cursor highlight were added
  in `apps/beach-web/src/components/BeachTerminal.tsx` and
  `apps/beach-web/src/terminal/cache.ts` (September 2025).
- Beach-Human host logic for cursor hints lives in
  `apps/beach-human/src/client/terminal.rs:1081` onwards.
- No automated coverage exists for cursor motion on the web client yet.


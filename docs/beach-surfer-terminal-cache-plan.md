# Beach Surfer Terminal Cache Plan

## Context

The web client currently applies updates directly to a sparse `Map` of row slots and renders by slicing that collection. This deviates from the Rust `GridRenderer`, which maintains:

- A dense ring buffer keyed by absolute row (`rows: Vec<RowSlot>`)
- Metadata about base row, scroll position, and follow-tail state
- Careful handling of trims, backfills, and pending/missing rows
- Style tables and sequence numbers for conflict resolution

Because the web client lacks this structure, we encounter issues:

1. **Duplicate history lines**: Without authoritative base-row tracking, the same logical row can appear multiple times (e.g. the "Restored session" banner).
2. **Incorrect viewport projection**: Rendering logic tries to mimic the Rust viewport but lacks the necessary cache guarantees.
3. **Limited backfill support**: Pending/missing rows are not represented consistently, making backfills hard to reason about.

## Goals

- Mirror the Rust grid cache semantics inside the web client
- Enable reliable viewport projections and history slicing
- Prepare for future features (selection, copy mode, backfill indicators)

## Proposed Architecture

### 1. Core Cache Module

Create `apps/beach-surfer/src/terminal/cache.ts` that exports a `TerminalGridCache` class with behaviours parallel to Rust's `TerminalGrid`/`GridRenderer` combo:

- Fixed-size ring buffer backing storage (configurable max history, default 5_000 rows)
- `baseRow`, `rows`, `cols`, `followTail`, `scrollTop`, `viewportHeight`
- Row slots: `{ kind: 'loaded' | 'pending' | 'missing', state?: { cells, latestSeq } }`
- Cell state structure: `{ char: string, styleId: number, seq: number }`
- Style table similar to Rust `StyleTable`

### 2. Update Application Pipeline

Port the primitives from Rust `GridRenderer`:

- `apply_cell`, `apply_row`, `apply_row_segment`, `apply_rect`
- Sequence comparisons to avoid overwriting newer data
- `apply_trim` to drop old rows and adjust `baseRow`
- Range helpers (`ensure_row`, `ensure_col`, gap detection)
- Observers for authoritative vs. speculative updates

This module should consume wire updates and output mutation events for the store/React layer.

### 3. Store Integration

Refactor `TerminalGridStore` to delegate to the cache:

- Hold a single `TerminalGridCache` instance
- Expose snapshot generation by reading the cache's row slots
- Maintain listeners and publish changes when the cache mutates
- Provide high-level operations (`setViewport`, `setFollowTail`, `markRowPending`) that forward to the cache

### 4. Rendering

The `buildLines` helper should become a thin adapter:

- Ask the cache for a `visibleRows()` slice (identical to Rust's method)
- Map row slots to renderable lines (trim trailing spaces, handle pending/missing placeholders)
- Avoid duplicating viewport math in the React layer

### 5. Backfill Support

Implement methods mirroring Rust:

- `mark_row_pending`, `mark_row_missing`, `first_gap_between`
- Tracking of `known_base_row`, `highest_loaded_row`
- Helpers for backfill controller (detect gaps, mark ranges, finalize ranges)

### 6. Testing Strategy

Add Vitest coverage mirroring Rust unit tests:

- Applying updates in sequence (cell -> row -> segment)
- Trims shifting base row
- Viewport following tail with pending rows trimmed
- Handling overlapping updates and seq guards

Use fixtures derived from Rust tests where possible to ensure parity.

### 7. Migration Plan

1. Introduce the cache module with unit tests (no integration yet)
2. Wire `TerminalGridStore` to the cache while keeping current API
3. Update `buildLines` to consume the cache's `visibleRows`
4. Validate behaviour end-to-end (manual session + Vitest)
5. Remove legacy map-based storage and temporary debug logs
6. Document the architecture in `docs/beach-surfer-terminal-cache-plan.md` (this file)

## Open Questions

- What history size is practical in the browser? (Evaluate memory usage; allow tuning)
- How to expose style table for theming? (Consider CSS custom properties for basic support)
- Do we need snapshot serialization (for reconnects/offline replay)? (Maybe later)

## Next Steps

- Implement `TerminalGridCache` skeleton with dense row storage
- Port update handlers (`apply_*`, `mark_row_*`, `observe_bounds`)
- Refactor store + renderer to depend on the cache API
- Remove temporary console debugging and window globals once confirmed working
- Expand test suite to cover viewport behaviour, trims, and backfills

This plan keeps the web client behaviour aligned with the mature Rust implementation, eliminating the current duplication bugs and providing a solid foundation for future features.

# Private Beach Rewrite · Changelog

## 2025-11-06 · Flow Canvas polish & coverage
- Reworked the page/shell layout and added a `ResizeObserver` gate so the React Flow surface always gets a real height before mount, eliminating the blank canvas warning.
- Synced React Flow node dimensions with tile resize events and tightened the tile preview CSS so the blue resize affordances track the terminal surface again.
- Added a node-level `ResizeObserver` to keep tile geometry aligned with live preview content, so the resize handles expand with the terminal instead of capping at the header.
- Manager snapshots now emit the full terminal history (plus `base_row`) so rewrite tiles hydrate every line instead of truncating the top of long sessions.
- Added a temporary fallback when `getBeachMeta` responds with 409 to keep the rewrite beach page usable while the upstream conflict is debugged.
- Introduced a Vitest harness for the rewrite app with focused `FlowCanvas` tests covering tile mapping and catalog drop snapping.

**Follow-ups**
- Trace and resolve the upstream 409 responses so the metadata fallback can be removed.
- Expand rewrite-side tests to cover tile resize/drag telemetry once the interaction surface stabilises.

## 2025-11-04 · React Flow Canvas Integration
- Replaced the bespoke DnD Kit canvas with the new `FlowCanvas` powered by React Flow, preserving tile focus, snapping, and telemetry payloads.
- Added HTML5 catalog → canvas drag and drop, grid-aware placement, and exhaustive React Flow logging for QA.
- Removed the temporary `NEXT_PUBLIC_REWRITE_CANVAS_FLOW` gate; React Flow now ships as the only rewrite canvas path.
- Removed the legacy `TileCanvas`/`TileNode` stack and `@dnd-kit/core` dependency after confirming behaviour parity.
- Updated canvas smoke tests to exercise the React Flow surface and catalog drag path.

**Follow-ups**
- Persist the React Flow viewport and pan state once the manager API exposes layout slots.
- Add keyboard-accessible move/resize affordances now that Flow owns the interaction layer.

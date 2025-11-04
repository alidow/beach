# Private Beach Rewrite · Changelog

## 2025-11-04 · React Flow Canvas Integration
- Replaced the bespoke DnD Kit canvas with the new `FlowCanvas` powered by React Flow, preserving tile focus, snapping, and telemetry payloads.
- Added HTML5 catalog → canvas drag and drop, grid-aware placement, and exhaustive React Flow logging for QA.
- Removed the temporary `NEXT_PUBLIC_REWRITE_CANVAS_FLOW` gate; React Flow now ships as the only rewrite canvas path.
- Removed the legacy `TileCanvas`/`TileNode` stack and `@dnd-kit/core` dependency after confirming behaviour parity.
- Updated canvas smoke tests to exercise the React Flow surface and catalog drag path.

**Follow-ups**
- Persist the React Flow viewport and pan state once the manager API exposes layout slots.
- Add keyboard-accessible move/resize affordances now that Flow owns the interaction layer.

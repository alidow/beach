# Private Beach Dashboard — Reconnect-On-Drop Issue

## Problem Statement

Dragging a tile on the Private Beach dashboard still produces a visible “connecting” blink when the tile is dropped. The blink coincides with a teardown/rebuild of the `SessionTerminalPreview` React subtree, which restarts the WebRTC terminal transport momentarily. Aside from the UX jolt, this reconnection interrupts streaming telemetry (viewport rows collapse to `1`, the terminal briefly reports `status: 'connecting'`, etc.).

### What’s Happening Today

- During a drag (`handleNodeDrag`) we call `sessionTileController.updateLayout('drag-preview-position', …)` on every mouse movement.
- Each of those mutations flows through `sessionTileController.replaceLayout`, which re-runs `syncStore(layout)` and `setNodes(nodes)` inside `CanvasSurface`.
- React Flow receives a brand-new node array and remounts the node (and our memoized tile component). The `SessionTerminalPreview` instance unmounts, logs `[terminal][diag] unmount`, and `viewerConnectionService` drops the transport.
- Drop completion (“drag-stop”) does the same workflow one more time, producing the visible blink exactly when the user releases the tile.

## Investigation Journey (abridged timeline)

1. **Initial bug report** (tile flickered heavily): discovered the controller was rehydrate-ing on every drag call. Added suppress-persist, hydrate-key guards, and DOM-measurement throttling. Reduced flicker but the reconnect-on-drop persisted.
2. Logs (`temp/private-beach.log`) showed repeated `[terminal][diag] unmount` / `mount` pairs keyed to drag operations, along with `[terminal][diag] viewer-change … status: 'connecting'`.
3. DOM measurement churn solved: we now suppress DOM telemetry while dragging/settling, so the viewport stays at PTY’s row/col count. Blink remained, proving the reconnection stemmed from component lifecycle, not measurement updates.
4. Reviewed React Flow usage: we always rebuild the entire node array (and call `setNodes`) when the layout changes; React Flow will treat these as new nodes even when the `id` is unchanged, triggering the remount.

## Goals

1. **No preview teardown when a tile is dropped.** Dragging may mute DOM telemetry, but the preview must continue streaming without flipping back to `connecting`.
2. **Keep layout persistence semantics.** When the user finishes a drag, the canonical layout should still update and persist via the controller.
3. **Minimise change footprint.** Keep the refactor within the Canvas/React Flow layer; avoid deeper rewrites of the session tile controller unless truly necessary.
4. **Retain drop-target affordances and drop scheduling.** Group creation, agent assignments, etc., should continue to work exactly as before.

## Evidence & Observations

| Evidence | Source | Notes |
| --- | --- | --- |
| `[terminal][diag] unmount` / `mount` pairs during drag | `temp/private-beach.log` lines 207, 777, 798… | Each drag induces multiple unmounts, confirming React remounts the preview component. |
| `[terminal][diag] viewer-change … status: 'connecting'` | `temp/private-beach.log` lines 203, 213 | Immediately follows preview unmount; transport resets to “connecting”. |
| `viewportRows: 1` pulses | `temp/private-beach.log` lines 696-704 | Occur during remount when DOM telemetry zeroes out; mitigated but still observable in logs. |
| Controller write cadence | `CanvasSurface.handleNodeDrag` | `sessionTileController.updateLayout('drag-preview-position', …)` fires every mousemove, feeding the rebuild loop. |
| Node update strategy | `CanvasSurface.syncStore` | Always calls `setNodes(nodes)`, replacing the entire array rather than mutating the existing nodes. |

## Proposed Refactor Plan

### 1. Stop mutating the shared layout during drag preview

- Remove the per-mousemove `sessionTileController.updateLayout('drag-preview-position', …)` call.
- Let React Flow’s internal drag state render the tile in its new position visually; continue tracking `hoverTarget` locally for drop target affordances.
- Keep any telemetry or hover state updates purely local to the Canvas component until drop.

### 2. Commit final position exactly once on drag stop

- Inside `handleNodeDragStop`, retain the existing `updateLayout('drag-stop-tile', …)` logic to write the new position and run drop/assignment workflows.
- This single update triggers the controller persist cycle, but only after the drag completes. Because we won’t rebuild nodes during the drag anymore, the preview remains mounted until this final commit—and even then, we’ll have steps in place to keep the component alive (see step 3).

### 3. Diff nodes instead of rebuilding them

- Refactor `syncStore` in `CanvasSurface` so it:
  - Loads the initial graph via `load` once (during hydrate).
  - On subsequent layout updates, iterates through the existing `nodes` array and only calls `updateNode(id, patch)` when necessary (position/size/metadata changed).
- React Flow will keep node instances (and their child component trees) intact when the same node reference is preserved; updating `data`/`position` in-place avoids remounts.
  - We can leverage the `updateNode` action already exposed by `useCanvasActions()` or patch the reducer to support partial updates.
  - Only when tile membership or node existence changes should we rebuild the graph array.

### 4. Track transient drag state locally

- Maintain `dragStateRef` or local state to control hover and drop target visuals.
- After drop, optionally run a short “settling” timeout (already in place) to suppress DOM telemetry until the React Flow animation finishes, but keep the component tree mounted throughout.

### 5. Regression guardrails

- Re-run draggable scenarios:
  - Drag & drop a tile (no group changes) → expect zero `[terminal][diag] mount/unmount` pairs, no `[terminal][diag] viewer-change … connecting`.
  - Drag tiles into/out of groups and onto agents → confirm drop logic still executes.
  - Verify layout persistence still posts to the API after `drag-stop`.
- Update automated tests if any rely on `drag-preview-position` events (adjust mocks accordingly).

## Implementation Notes

- The biggest code change is around `syncStore` and the drag preview path in `CanvasSurface.tsx`.
- We may add a helper to diff the controller layout vs. the React Flow store; e.g., build a `Map` keyed by node id to compare positions, widths, etc., and dispatch targeted updates.
- Ensure the controller still broadcasts layout changes for persistence/side effects—only the view layer will throttle updates.
- After the refactor, the log should show a single `viewer-change` (connecting → connected) on initial tile mount; no additional entries should appear during subsequent drags.

## Next Steps

1. Remove the drag-preview layout mutation and rely on React Flow for temporary visuals.
2. Refactor `syncStore` to perform node-level diffs (use `updateNode` reducer action).
3. Re-test with telemetry logging enabled; confirm no reconnect bloom.
4. Capture before/after traces and ship the fix with documentation (update this doc + release notes).

---

**Owner:** `@codex`  
**Last Updated:** 2025-02-02  
**Related Logs:** `temp/private-beach.log`, `/tmp/beach-host.log`

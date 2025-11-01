# Private Beach Tile Zoom & Resize Redesign

## Goals
- Restore a compact tile-first dashboard view where every tile defaults to a small footprint (~400 px max-dimension) and shows as much of the PTY as possible.
- Move zoom control to the tile level so each session can be tuned independently.
- Clarify and streamline the tile toolbar: quick “match host size”, a lock toggle that synchronises PTY size with tile size, fullscreen, and details.
- Make resizing intuitive:
  - In **unlocked** mode the user is only changing zoom; the in-tile PTY scales without asking the host to resize.
  - Aspect ratio stays bound to the host PTY while unlocked.
  - When the tile reaches the host PTY’s full size, further resizing is disabled unless the user locks and thereby opts into resizing the host PTY.
  - In **locked** mode the host PTY follows tile dimension changes.
- Keep the toolbar visually light and optionally hideable.

## Experience Breakdown

### Default Tile Placement
- When a tile is attached, clamp its grid layout to approx. **320×200** (final dims tuned to the grid system) with a hard **max width/height of ~400 px**.
- Compute an initial zoom multiplier so the full PTY (cols/rows from host metadata) fits inside the tile. Cap at 80×80; if the host PTY exceeds those bounds crop/overflow inside the canvas (no scrollbars) and show a subtle “cropped” indicator.
- Persist tile footprint and zoom per session in the existing `layout` snapshot; introduce new metadata for zoom + lock state.

### Toolbar (per tile)
Structure: `[session id chip] [snap icon] [lock toggle] [expand] [details]`.

| Control | Behaviour |
| --- | --- |
| Session ID chip | First 8 chars; hover reveals full id. Acts as drag handle. |
| Snap icon | Visible when host PTY dims are known. Clicking resizes the tile to exactly match host PTY (within layout bounds) and synchronises zoom to 1. |
| Lock toggle | Disabled until tile ≥ host PTY size. Unlock state = zoom-only; Locked state = host PTY resize. On first lock, send resize to match current tile bounds. Tooltip clarifies mode. |
| Expand | Opens fullscreen with zoom reset to 1 while in fullscreen; reapply previous zoom on exit. |
| Details | Opens drawer (existing behaviour). |

Toolbar presentation:
- Single 32 px height, translucent background, gradient fade to minimise visual weight.
- Auto-hide option: hover/focus reveals; keyboard shortcut (e.g. `t`) toggles persistent visibility.

### Resizing Rules
- **Unlocked (default):**
  - Tile resize updates a `tileZoom` factor (tile size ÷ host PTY size).
  - Maintain PTY aspect ratio by restricting resize handles to corner dragging; intercept `react-grid-layout` resizes to keep width/height ratio constant.
  - When tile reaches host PTY dimensions within a small epsilon, clamp further resizing and show hint “Lock to resize host”.
- **Locked:**
  - Allow full resizing (still aspect-ratio constrained by user modifier? TBD). Compute target PTY cols/rows from tile size, send `resize` to host after debounce.
  - Update stored host PTY dims after confirmation to keep zoom = 1 while locked.
- Provide visual feedback (badge or icon glow) when zoomed vs locked, and show cropping indicator if tile smaller than host dims.

### Per-Tile State Model
- Extend `BeachLayoutItem` snapshot with:
  ```ts
  type BeachTileViewState = {
    id: string;
    widthPx: number;
    heightPx: number;
    zoom: number;
    locked: boolean;
    toolbarPinned: boolean;
  };
  ```
- Persist alongside existing layout in `putBeachLayout`.
- At runtime maintain a React state map keyed by session id for zoom + lock + toolbar visibility.

## Implementation Plan

### Phase 1 – Data & State Prep
1. Add optional fields to layout persistence schema (`BeachLayoutItem`) and manager API payloads. Backfill defaults when missing.
2. Update `TileCanvas` to:
   - Initialise small tile dimensions on add.
   - Track per-tile zoom/lock state with React state seeded from saved layout.
   - Provide callbacks for toolbar controls.

3. Extend `SessionSummary` fetch or controller snapshots (`sessionTileController` / `viewerConnectionService`) to expose host PTY cols/rows (if not already surfaced) for snap/lock decisions.

### Phase 2 – UX Scaffolding
1. Replace top-nav zoom select.
2. Rebuild tile header into minimal control strip. Add icons (likely using Lucide set already in project).
3. Implement auto-hide behaviour (CSS + focus-visible support).

### Phase 3 – Zoom Mechanics
1. Update `SessionTerminalPreviewClient`:
   - Accept `zoom`, `locked`, `hostDimensions`.
   - When unlocked, scale font size + line height to achieve zoom (no container scaling).
   - When locked, enforce zoom=1 and expose helper to compute PTY resize targets.
   - Provide cropping overlay if host > tile.
2. Adjust `BeachTerminal` container to accept explicit viewport size driven by tile bounding box when locked.
3. In `TileCanvas`, intercept resize events:
   - When unlocked: calculate new zoom from tile size while keeping aspect ratio, stop at host size.
   - When locked: derive host cols/rows, debounce `resize` command via existing host resize control.

### Phase 4 – Persistence & Sync
1. On zoom/lock change, update snapshot and call `onLayoutPersist`.
2. Ensure fullscreen view:
   - Temporarily overrides zoom to 1.
   - Restores previous zoom on exit without persisting.

### Phase 5 – Polish & QA
1. Visually tune toolbar alignment, icon states, hover/focus styles.
2. Add unit tests:
   - Toolbar state transitions (snap, lock).
   - Zoom calculations within `TileCanvas`.
   - `SessionTerminalPreviewClient` scaling logic and cropping indicator.
3. Add Playwright regression for resize/lock workflow.

### Open Questions
- Exact tile-gutter handling when multiple tiles snap to host dimensions (ensure grid still looks neat).
- Accessibility: keyboard shortcuts for snap/lock? aria descriptions for auto-hide toolbar?
- Debounce strategy for locked host resize (avoid spamming backend).

## Rollout Considerations
- Migration script to populate defaults for existing saved layouts (zoom = min(tile/host, 1)).
- Feature flag gating in case Tiles view is shared with other teams.
- Communicate toolbar changes in release notes / docs.

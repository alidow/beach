# Private Beach Session Tile – Live Preview Plan

## Goals
- Replace the `Live stream placeholder` inside `TileCanvas` with an actual miniature terminal (and future harness) preview that reflects real-time state.
- Allow members to arrange session tiles into an ad-hoc dashboard: drag to reposition, resize tiles to emphasize key sessions, and collapse/expand without losing layout.
- Provide a seamless way to pop any tile into a full-session view that reuses the existing Beach Surfer experience, then return to the dashboard with state preserved.

## Current State
- `apps/private-beach/src/components/TileCanvas.tsx` renders a static grid with fixed-height cards; the center region is a placeholder div.
- Live terminal rendering, transport wiring, and layout persistence logic all exist today in Beach Surfer (`apps/beach-surfer`). Rather than re-implement those concepts, we should extract and reuse them.
- Layout choices are hard-coded (`gridCols` heuristics). There is no drag/drop, resizing, or persisted arrangement.

## Component & Transport Reuse Strategy
1. **Extract shared terminal primitives**
   - Move `BeachTerminal`, `BeachViewer`, terminal grid store hooks, and transport adapters (`terminalTransport.ts`, WebRTC handshake helpers) into a shared package (e.g., `packages/beach-shell`).
   - Ensure the shared package publishes React components (for Next.js) and headless stores usable from both Surfer and Private Beach apps.
   - Export a light-weight `TerminalPreview` wrapper that accepts a `TerminalTransport` or `TerminalSnapshot` and renders a scaled terminal grid without the Surfer toolbar chrome.

2. **Bridge Manager state to terminal transports**
   - For terminals already using WebRTC fast-path: reuse the `WebRtcTransport` handshake from Surfer (fast-path endpoints already exist in Manager). Tiles can connect via read-only credentials, mirroring the full viewer.
   - Until fast-path is available, fall back to SSE/HTTP state snapshots:
     - Implement a `ManagerTerminalFeed` that watches `GET /sessions/:id/state/stream` events, rehydrates the terminal grid state (the payload already contains diff frames), and pushes updates into the shared terminal store (`applyTerminalFrame` helper from Surfer).
     - The bridge satisfies the `TerminalTransport` interface by emitting decoded host frames and ignoring outbound `send` requests (tiles are read-only). This keeps the preview compatible with the shared terminal component.

3. **SessionTile composition**
   - Build a `SessionTileTerminal` component in `apps/private-beach` that:
     - Creates/receives a shared terminal store instance (one per session tile when mounted).
     - Subscribes to Manager events via the new feed bridge.
     - Renders the miniature terminal via the shared `TerminalPreview`.
     - Shows basic overlays (health badge, pending actions) in the tile chrome.
   - For non-terminal harnesses (e.g., Cabana), add pluggable renderers; the plan should default to a placeholder badge until those viewers are extracted.

## Layout, Resizing, and Persistence
1. **Layout data model**
   - Represent each tile with `{ sessionId, w, h, x, y, minimized }`, where `w/h` are grid units (e.g., 1–4), and `x/y` map to positions on a virtual grid.
   - Maintain layout state in React (and later persist to Manager via `/private-beaches/:id/layout` once that API lands). For now, persist per-beach layout to `localStorage` to survive reloads.

2. **Interaction primitives**
   - Adopt `react-grid-layout` (MIT) or an equivalent headless grid+resize library to provide drag-and-drop ordering and corner handles. This library manages collision resolution and responsive breakpoints.
   - Wrap `TileCanvas` in the grid layout component; feed layout state via controlled props.
   - Render resize handles using Tailwind classes for visual affordances; snap resizing to grid units (e.g., 120px columns/rows) to keep terminals legible.

3. **Responsive behavior**
   - Define breakpoint configs: single-column stack on small screens, 12-column grid on desktop. Smaller breakpoints can auto-collapse wide tiles to full width.
   - When screen size changes, re-run layout compaction from the grid library and update stored state.

4. **Persistence**
   - After every drag/resize, debounce and save layout to `localStorage` (keyed by beach ID plus session set hash).
   - Provide a “Reset layout” action (per beach) to fall back to presets (`grid2x2`, `onePlusThree`, `focus`).
   - Once Manager exposes layout CRUD, replace local persistence with API calls and optimistic updates.

## Full-Screen & Expansion UX
1. **Tile chrome**
   - Add actions in the tile header: `Expand`, `Pop out`, `Remove`.
   - `Expand` toggles the tile into a maximized state that covers the dashboard (CSS absolute overlay). Layout metadata should remember the previous size to restore on collapse.
   - `Pop out` opens a modal or route (`/beaches/:id/sessions/:sessionId`) that mounts the full Beach Viewer with controls; share the same terminal store so the tile stays warm (no reconnect).

2. **Full-screen implementation**
   - Use a fullscreen modal (`Dialog` from shadcn/ui) with the shared `BeachViewer` component, controller buttons, and metadata. Background tiles remain mounted but visually dimmed, preserving their state.
   - Keyboard shortcuts: `Esc` exits fullscreen, `Ctrl+Enter` toggles maximize for the focused tile.

3. **State & performance considerations**
   - Keep the terminal store alive while switching between tile and full-screen modes to avoid renegotiating transports.
   - Pause unnecessary renders: when a tile is not visible (e.g., behind another overlay), throttle diff application or skip repainting the preview canvas.

## Implementation Phases
1. **Foundation**
   - Extract/shared terminal components + transport utilities.
   - Create `ManagerTerminalFeed` adapter for SSE fallback.
   - Render a static grid tile with live terminal preview (no drag/resize yet) to verify transport reuse.

2. **Interactive layout**
   - Introduce grid layout library, control drag/resize, persist to `localStorage`.
   - Adjust `TileCanvas` API to accept layout state; fall back to presets when no layout stored.
   - Add visual affordances (grid background, resize handles, hover outlines).

3. **Full-screen UX**
   - Add tile header actions, overlay maximize mode, and modal-based full viewer.
   - Share terminal stores between tile and full-screen view; ensure controller controls remain responsive.
   - Telemetry: log expand/collapse events and reconnect counts for monitoring.

4. **Polish & Manager sync**
   - Persist layout via Manager layout endpoints when available.
   - Support additional harness types by extracting Cabana/web widgets into shared components.
   - Accessibility pass: keyboard drag/resize, focus rings, aria labels.

## Open Questions
- Do we want concurrent renders of the same session (tile + full viewer) to share a single transport, or should the full viewer request control (with fallback for read-only tiles)?
- How aggressively should previews throttle diff repainting to keep the dashboard smooth when many tiles stream simultaneously?
- Should layout edits be gated by a lock/edit mode to prevent accidental drags during busy operations?
- Which telemetry signals (e.g., tile fps, reconnect rate) do we need to ensure the experience scales beyond 4–6 concurrent tiles?

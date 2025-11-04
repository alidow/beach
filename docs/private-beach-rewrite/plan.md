# Private Beach Rewrite Plan

## 1. Objectives & Principles
- Deliver a maintainable Next.js/React dashboard that replaces the current canvas implementation without disrupting `/beaches` list flows.
- Prioritise simplicity, predictable sizing, and minimal hidden state; prefer explicit dimensions and declarative data.
- Facilitate incremental rollout (feature flag or alternate route) so current users retain access until the rewrite hits parity.
- Optimise for collaboration: ensure tasks can be split among Codex workers with clear checkpoints and ownership boundaries.

## 2. Scope & Non-goals
**In scope**
- New Next.js app in `apps/private-beach-rewrite/` sharing auth/session APIs with the existing backend.
- Simplified beach dashboard with fixed-size tiles, drag-and-drop node catalog, manual resizing, and session connection form.
- Integration of the existing terminal/preview client once the new canvas shell is stable.
- Basic analytics/telemetry hooks to monitor tile usage and connection status.

**Out of scope for v1**
- Major changes to `/beaches` list backend or data model (reuse existing endpoints).
- Advanced node types beyond "Application".
- Automatic layout optimisation, zoom-to-fit, or collaborative editing.
- Comprehensive design system refactor (use lightweight styling aligned with existing brand, but no full design overhaul).

## 3. Architecture Overview
### 3.1 App Shell & Routing
- Wrap the rewrite inside a new Next.js app; expose `/beaches` list (shared component) and `/beaches/[id]` rewrite page.
- Use Next.js App Router if feasible; otherwise stay with Pages Router for parity with current project structure.
- Implement lightweight top nav with back-to-list control and contextual beach/session metadata fetched via existing API clients.

### 3.2 Canvas Layout
- Main canvas uses CSS grid or absolute positioning inside a relative container; choose the simpler approach (absolute `position` with stored x/y coordinates) for MVP.
- Right-side collapsible drawer contains draggable node cards; maintain open/closed state in React context.

### 3.3 Nodes & Tiles
- Each tile records: `id`, `nodeType`, `position {x,y}`, `size {width,height}`, `sessionMeta`.
- Tile header shows truncated session id, provides close (`X`) button, and exposes resize handles on hover.
- Tile body hosts scrollable content; terminal/preview area sits inside without scaling—overflow scrollbars reveal hidden regions.

### 3.4 Drag & Drop & Resizing
- Prefer `@dnd-kit/core` for composable drag sensors; fallback to native drag/drop if bundle size matters.
- For placement: translate drop point into canvas coordinates, snap to 8px grid for consistency.
- Resizing: custom mouse handlers on tile edges that update size in pixels; maintain minimum dimensions to prevent collapse.

### 3.5 Session Connection Flow
- Within an Application tile, render form for session id + passcode; on submit, call existing connect endpoint.
- Once connected, swap form with `SessionTerminalPreviewClient` (or a thin wrapper) and display connection status indicator.
- Handle reconnect/backoff logic in a dedicated hook reused from the current app where possible.

### 3.6 State Management & Persistence
- Maintain canvas state in React context or Zustand store to avoid prop drilling.
- Support future persistence by defining a serialization shape, but keep MVP ephemeral; store layout in browser `localStorage` behind a toggle.
- Expose async actions (add tile, remove tile, resize tile) via typed hooks for testability.

### 3.7 Styling & Theming
- Use Tailwind or CSS Modules for predictable styling; avoid runtime styling churn.
- Keep components accessible (e.g., focus traps for drawer, keyboard support for delete/resize toggles where feasible).

### 3.8 Testing & Observability
- Unit-test store reducers/hooks for tile operations.
- Introduce Playwright smoke tests for drag/drop, resize, and session connect flows.
- Emit analytics events for tile create/delete/connect to feed into existing metrics pipeline.

## 4. Implementation Roadmap & Milestones
1. **Scaffolding**: create `apps/private-beach-rewrite/`, configure TypeScript, ESLint, shared env, and integrate existing session API clients.
2. **Navigation & Beach Page Shell**: implement `/beaches` list reuse, `/beaches/[id]` layout, and top nav.
3. **Canvas Skeleton**: render canvas surface, collapsible node catalog, and placeholder drop zones.
4. **Tile Lifecycle**: enable drag/drop to spawn Application tiles, provide delete action, and persist layout in client state.
5. **Resizing & Scroll Behavior**: add resize handles with visual feedback; ensure scrollbars handle overflow gracefully.
6. **Session Integration**: implement connection form, wire to terminal preview, handle error/success states, and maintain connection status.
7. **Telemetry & QA**: add instrumentation, smoke tests, accessibility audit, and polish UX (loading states, keyboard behavior).
8. **Launch Prep**: document rollout strategy, feature flagging, and migration plan from legacy canvas.

Dependencies allow parallelisation after Milestone 2: once scaffolding and shell exist, tile lifecycle, resizing, and session integration can progress concurrently with testing/telemetry preparations.

## 5. Workstreams & Parallel Execution
| Workstream | Focus | Key Deliverables | Dependencies | Parallelisation Notes |
| --- | --- | --- | --- | --- |
| WS-A: App Bootstrap & Shared Infrastructure | Next.js app setup, shared config, API client integration | New app skeleton, shared env config, base layout components | None | Must finish before most other streams; keep documentation in `docs/private-beach-rewrite/ws-a.md`. |
| WS-B: UI Shell & Navigation | `/beaches/[id]` layout, top nav, beach metadata loading | Back-navigation, header, loading skeleton | WS-A | Can overlap with WS-C once layout shell in place. |
| WS-C: Canvas & Node Catalog | Canvas container, drawer interactions, drag/drop scaffolding | Collapsible node catalog, drop target management | WS-A (basic app) | Can proceed while WS-B finalises nav details; coordinate on layout breakpoints. |
| WS-D: Tile Lifecycle & Resizing | Tile state store, creation/removal, resize handles, scrolling | Tile reducer/hooks, UI components with resizing | WS-C (canvas scaffolding) | Work closely with WS-C to align drag/drop events and tile placement data. |
| WS-E: Session Integration | Application tile form, connection handling, terminal embed | API hook, success/error states, live preview | WS-D (tile component shell) | Begin mock implementation early, swap to real client once WS-A exposes API utilities. |
| WS-F: QA, Telemetry & Rollout | Testing, analytics events, feature flagging, docs | Playwright suite, analytics hooks, release doc | WS-A (tooling), WS-E (connect flow) | Runs throughout; coordinate with all streams to ensure instrumentation coverage. |

Each workstream maintains a short progress log (see Section 7) and raises cross-stream blocking issues in shared sync notes.

## 6. Codex Worker Prompts
- **WS-A Prompt**  
  ```
  You're handling WS-A (App Bootstrap & Shared Infrastructure) for private-beach rewrite. Deliver a Next.js app in apps/private-beach-rewrite/ with shared config, TypeScript/ESLint setup, and exported API client utilities reused from the existing app. Follow docs/private-beach-rewrite/plan.md sections 3.1 and 4 Milestones 1-2. Track progress in docs/private-beach-rewrite/ws-a.md using the template provided.
  ```
- **WS-B Prompt**  
  ```
  You're assigned WS-B (UI Shell & Navigation). Starting from the scaffolded rewrite app, implement the beaches list reuse and the /beaches/[id] page shell with top navigation and beach metadata loading. Align with layout requirements in Section 3.1-3.2 and Milestones 2-3 of docs/private-beach-rewrite/plan.md. Log updates in docs/private-beach-rewrite/ws-b.md.
  ```
- **WS-C Prompt**  
  ```
  You're owning WS-C (Canvas & Node Catalog). Build the canvas container and collapsible node drawer, add drag-and-drop scaffolding based on Section 3.2-3.4. Coordinate canvas sizing contracts with WS-D via shared notes in docs/private-beach-rewrite/sync-log.md. Capture your progress in docs/private-beach-rewrite/ws-c.md.
  ```
- **WS-D Prompt**  
  ```
  You're responsible for WS-D (Tile Lifecycle & Resizing). Implement tile state stores, creation/deletion handlers, and resize interactions per Sections 3.3-3.5 and Milestones 4-5. Work with WS-C on drag/drop data structures. Document progress in docs/private-beach-rewrite/ws-d.md.
  ```
- **WS-E Prompt**  
  ```
  You're executing WS-E (Session Integration). Inside the Application tile, create the connect form, wire it to reuse existing session APIs, embed the terminal preview client, and handle connection states as defined in Sections 3.5-3.6 and Milestone 6. Keep notes in docs/private-beach-rewrite/ws-e.md and sync blockers in docs/private-beach-rewrite/sync-log.md.
  ```
- **WS-F Prompt**  
  ```
  You're covering WS-F (QA, Telemetry & Rollout). Add targeted unit tests, Playwright smoke coverage, analytics hooks, and feature flag strategy per Sections 3.8 and Milestones 7-8. Update docs/private-beach-rewrite/ws-f.md and coordinate release criteria across streams via docs/private-beach-rewrite/sync-log.md.
  ```

## 7. Progress Tracking & Sync Rituals
- Each workstream owns a log file (`docs/private-beach-rewrite/ws-<id>.md`) with the following template:
  ```md
  # WS-<ID> Progress Log
  - **Owner**: <Name>
  - **Last updated**: <ISO timestamp>
  - **Current focus**: …
  - **Done**:
    - …
  - **Next**:
    - …
  - **Blockers/Risks**:
    - …
  ```
- Shared coordination lives in `docs/private-beach-rewrite/sync-log.md`, updated after each async sync or when raising blockers that affect multiple streams.
- Weekly checkpoint: each owner posts summary in sync log referencing their latest progress logs and outstanding dependencies.

## 8. Risks & Open Questions
- **Terminal Client Coupling**: ensure the existing `SessionTerminalPreviewClient` does not assume legacy layout contracts; may need wrapper component or refactor.
- **State Persistence Expectations**: confirm with stakeholders whether layouts must persist across sessions before investing in server persistence.
- **Drag/Resize Performance**: monitor for jank, especially if multiple tiles render live terminal feeds; plan virtualization if needed.
- **Launch Strategy**: decide between feature flag vs. dedicated beta environment early to avoid last-minute deployment surprises.

Open questions should be tracked in the sync log with owners and resolution deadlines.

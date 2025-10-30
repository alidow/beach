# Controller & Application UX Refresh Plan

Owner: Codex — 2025-07-04  
Status: Draft (ready for engineering breakdown)

## Objectives
- Replace the ad-hoc “controller pairing” UI with a clear agent ⇆ application model that scales to desktop and mobile.
- Make controller assignments discoverable, editable, and resilient without modal churn.
- Eliminate the old `Acquire / Release / Stop / Pair` affordances while keeping critical operations intuitive.
- Prepare the interaction model for eventual native clients by standardising gestures and panes.

## Terminology & Entities
- **Agent session** — any session allowed to control other sessions. Agents can themselves be controlled by other agents (future-proofing).
- **Application session** — default session type; can be controlled by one or more agents.
- **Assignment** — relationship between an agent (controller) and an application (controlled). Carries metadata: initial prompt, cadence, transport status, etc.
- **Workspace** — a private beach; contains agents and applications.

## High-Level Experience
1. **Attach flow** asks whether the new session is an agent or application (defaulting to application) with a short helper description. Users can change this later.
2. **Tiles** become primarily observational:
   - No Acquire/Release/Stop buttons.
   - Agent tiles display a collapsible “Assignments bar” showing thumbnails of each controlled application.
   - Tapping a thumbnail expands the full application tile in-place (desktop) or brings its preview to the forefront (mobile).
3. **Left sidebar** becomes a multi-facet explorer:
   - Switchable facets for `Collections`, `Groups`, and `Saved Views` keep large session inventories manageable.
   - Collections include system buckets (Agents, Applications, Observers, Archived) with lazy loading and alphabetical chunking.
   - Groups support user- or org-defined folders and future smart grouping rules; saved views pin commonly used filters.
   - Drag + drop (or long-press + move on mobile) assigns children to agents or reorders within groups; keyboard fallback uses an “Assign to…” action and `Cmd+G` for grouping.
4. **Right detail pane** slides out when creating/editing an assignment (instead of modal). Houses prompt text, cadence, transport status, history, and destructive actions.
5. **Global overview** (small header strip) surfaces counts of agents, applications, and active assignments but defers deeper inspection to the sidebar + pane.

## Detailed Interaction Model

### Session Attach & Type Management
- **Attach modal**: introduces a toggle `Agent | Application`. Include helper copy:
  - Agent: “Can drive other sessions; requires prompts and cadence.”
  - Application: “Runs tasks and can be controlled by agents.”
- **Tile toggle**: each tile header includes a contextual menu (`⋯`) with “Convert to Agent/Application”. Converting updates sidebar grouping and agent bar visibility.
- **Validation**: if converting an agent to application, prompt user to detach assignments or confirm that they will be removed automatically (final behaviour TBD).

### Tile Layout
- **Header**: retains ID badge, harness type, health badges. Replace “Pair” button with `⋯` menu (convert type, open in drawer, remove).
- **Body**: same terminal preview.
- **Assignments bar (agents only)**:
  - Default collapsed state shows pill with assignment count.
  - Expanded view lays out micro-thumbnails (70 × 60 px desktop, 48 × 48 px mobile) representing each controlled application.
  - Each micro-tile shows application alias, transport status icon, and “tap to focus”.
  - Collapse button uses chevron icon; state persists per agent.
- **Application tile indicator**: small stack of agent chips (avatars or initials) in the header to show who controls it.

### Explorer Navigation Model
- **Top-Level Layout**:
  - Replace the static two-section list with a three-column facet header: `Collections`, `Groups`, `Saved Views`. Only one facet is expanded at a time; the others collapse to headers with counters.
  - Default landing view is `Collections`, showing system-defined trees for `Agents`, `Applications`, `Observers` (future live viewers), and `Archived`. Each collection is lazily populated to support thousands of nodes.
  - `Groups` lists user-curated or org-synced bundles (e.g., “Infra agents”, “Pong demo sessions”) and will eventually support nested groups. Groups behave like folders: expand to reveal contained agents/applications, with breadcrumbs to show membership hierarchy.
  - `Saved Views` capture filtered slices (query + sort) and pin them at the top. Selecting a saved view applies its filter state to the list below without changing the facet structure.
- **Node Anatomy**:
  - Each row uses compact density (28–32 px tall) with icons for harness type (CLI, Cabana, Agent), alias, status dot, and optional meta chips (e.g., controller count, region).
  - Secondary line (visible on hover/focus) shows last update timestamp and abbreviated prompt/job snippet for agents.
  - Badges stack horizontally and truncate gracefully; hovering reveals the full text in a tooltip.
- **Grouping & Large Sets**:
  - Within a collection/group, nodes may subdivide by derived headings (e.g., alphabetical buckets `A–C`, `D–F` once the list exceeds 200 items) to reduce scroll fatigue.
  - Multi-assigned applications appear under every controlling agent but include a `controller xN` chip that opens a popover listing controllers with jump links.
  - Agents with many children (>20) show a summarized child count; expanding reveals a paginated/virtualized list of assignments with quick filters (`active`, `paused`, `errors`).
- **Interactions**:
  - Hover or keyboard focus reveals checkboxes for multi-select; `Shift+Click` creates ranges, `Cmd/Ctrl+Click` toggles individuals. Sticky action bar appears at the top of the panel with context-aware commands (Assign, Open history, Detach, Move to group).
  - Drag & drop is routed via clear drop affordances: folders highlight in blue, agents show “Assign N sessions” hint, and saved views reject drops with a shake animation.
  - Right-click (or long-press) opens a contextual menu with primary actions plus “Open details in pane”. Menu respects selection state; if multiple nodes are selected, bulk actions surface first.
  - Quick-search field (⌘P) sits above the tree, providing fuzzy search across alias, session id, group, and label metadata. Results appear inline; hitting `Enter` focuses the highlighted node.
- **Performance & Scaling**:
  - Long lists are virtualized with sentinel-based incremental fetching. Scroll position per collection/group is cached so switching facets does not reset context.
  - API supports delta updates (SSE) to insert/move nodes without rerendering the entire tree. UI batches updates to avoid thrash during bursts; a subtle toast indicates “12 sessions updated” with undo to clear filters.
  - Collapsed groups persist across sessions through user preferences stored server-side.
- **Accessibility**:
  - Tree grid pattern with ARIA attributes communicates hierarchy depth, expanded state, and selection count.
  - Keyboard shortcuts: `←/→` collapse/expand, `Enter` focuses selected tile, `Space` toggles selection, `Shift+Enter` opens the detail pane, `Cmd+G` creates a new group from selected nodes.
  - Live region announces assignment changes (“Pong Manager now controls Pong Left and Pong Right”) and surface errors (“Assign failed: agent rejected request”).
- **Future Groupings**:
  - Support org-level auto groups (e.g., “Team: SRE”, “Project: Beach Cabana”) once metadata is available; groups can include agents, applications, or nested groups.
  - Introduce smart groups (rule-based) where users define conditions (`role=application AND label:demo`) that update automatically. Smart groups display a gear icon to indicate managed membership.
  - Provide merge/split operations for groups so operators can reorganize at scale without reassigning individually.

### Assignment Editing Pane (Right Side)
- **Invocation**:
  - Automatically opens after drag/drop assignment creation.
  - Accessible via sidebar context menu or micro-tile click.
- **Content**:
  - Top summary: agent ↔ application, transport status chip, last updated.
  - Tabs or stacked sections: `Prompt`, `Cadence`, `Transport`, `Notes`.
  - Inline status timeline (recent events).
  - Actions: “Disable assignment”, “Duplicate settings to…”, “Remove assignment”.
  - Close button returns focus to sidebar selection.
- **Responsive behaviour**:
  - On wide screens, pane slides over 30% width.
  - On mobile, pane becomes a full-screen sheet with swipe-to-close.

### Mobile Considerations
- Sidebar collapses to a drawer accessible via top-left menu icon; explorer tree uses accordion pattern with clear assign CTA.
- Drag & drop replaced by long-press + action sheet; micro-tiles respond to tap to open application detail; pinch gestures ignored to prevent accidental zoom.
- Right pane uses full-screen overlay with large tap targets (44 × 44 px minimum).
- Tile grid becomes single column or two-column depending on viewport width; assignments bar should adapt to horizontal pill list with swipe when space is constrained.

### Accessibility & Feedback
- All drag actions have menu equivalents; assignments announce via ARIA live region (“Agent A now controls App Beta”).
- Keyboard shortcuts: `a` to focus Agents list, `p` to assign selected application to last-used agent, `Shift+A` to add new agent.
- Toasts replaced with anchored inline confirmations (e.g., checkmark next to agent entry) so feedback remains visible even without toast stack.

## Data Model Implications
- Sessions require a `role` enum (`agent`, `application`, `hybrid` reserved).
- Assignment schema must support multiple agents per application; ensure ordering metadata for sidebar display.
- Store user-specific layout prefs: assignment bar collapsed state, sidebar expansion, sort order.
- Transport status already handled; ensure SSE events map to new UI structures.

## Implementation Roadmap

1. **Backend**  
   - Add `role` field to sessions API, update attach endpoints, extend manager CLI scaffolding.  
   - Permit multiple controllers per application at the API level (ensure create endpoint handles duplicates gracefully).  
   - Extend SSE payloads with stable metadata required by sidebar (e.g., agent display name, assignment order).

2. **Shared Components**  
   - Create `useAssignments` hook returning normalised agent/application trees.  
   - Refactor pairing modal to a reusable `AssignmentPane` component.  
   - Build micro-tile component with shared styling tokens.

3. **Page Layout**  
   - Replace current `SessionListPanel` with explorer tree.  
   - Introduce top-level layout state for sidebar + pane.  
   - Update tile canvas to support agent assignment bar and remove old buttons/modals.

4. **Mobile/Responsive**  
   - Implement drawer pattern for sidebar.  
   - Audit Breakpoints: ensure tile stacking and pane transitions feel natural.

5. **QA & Rollout**  
   - Update Vitest/Playwright suites to cover explorer interactions, assignment pane, SSE updates.  
   - Revise docs and in-app guides to introduce new terminology.  
   - Feature flag initial rollout to allow fallback if needed.

## Open Questions
- Hybrid sessions: do we need an explicit role that can both control and be controlled simultaneously? If so, clarify UI representation.  
- Assignment defaults: should dragging inherit prompt/cadence from last assignment by that agent?  
- Conflict handling: when removing agent role from a session with active assignments, do we auto-delete assignments or prompt to reassign?

---

This document is intended to be the handoff reference for design + engineering; keep it updated as we answer the open questions and begin implementation.*** End Patch

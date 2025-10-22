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
3. **Left sidebar** behaves like an explorer tree:
   - Top-level groups: “Agents” and “Applications”.
   - Agents expand to show all applications they currently control.
   - Applications may appear under multiple agents to reflect multi-controller assignments; each entry is annotated with a small badge (e.g. `Agent A`).
   - Drag + drop (or long-press + move on mobile) assigns applications to agents. Keyboard fallback uses “Assign to…” action.
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

### Sidebar Explorer
- **Structure**:
  ```
  Agents
    Agent A
      ▸ App Alpha
      ▸ App Beta
    Agent B
      ▸ App Beta
      ▸ App Gamma
  Applications
    App Alpha (2 controllers)
    App Beta (1 controller)
    App Gamma (unassigned)
  ```
- **Badges**:
  - Under Agents: show transport status dot + prompt snippet on hover (desktop) or in detail pane.
  - Under Applications: show count of controllers; grey out when unassigned.
- **Interactions**:
  - Drag application node onto agent node to create assignment (desktop).
  - On mobile: long-press application to open action sheet (`Assign to…`) listing agents; selecting one creates assignment.
  - Keyboard: `Enter` on application opens quick actions menu with “Assign to…”.
  - Agents support “bulk assign”: dropping multiple selected applications (multi-select via checkboxes or shift-click on desktop) creates multiple assignments with shared initial prompt defaults.
- **Reordering**: Users can reorder agents and applications within their sections; persists in user preferences.

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

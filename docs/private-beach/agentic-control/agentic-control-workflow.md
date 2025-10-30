# Agentic Control Workflow Plan

Owner: Codex — 2025-07-06  
Status: Draft (ready for engineering breakdown)

## Goals
- Deliver a predictable workflow for attaching application sessions to controlling agents inside the Private Beach dashboard.
- Keep prompt authoring lightweight for users while ensuring agents receive the structured context required to operate safely.
- Make controller ⇆ child relationships discoverable, editable, and auditable across desktop, mobile, and accessible input modes.
- Surface streaming activity and history in ways that help users trust—and, when needed, override—agent behaviour.

## Glossary
- **Agent session** — a session permitted to orchestrate other sessions via MCP tools and `beach action` CLI verbs.
- **Application session** — a controllable session (CLI, GUI, or another agent) that exposes state streams an agent can consume.
- **Assignment** — the relationship between a controller agent and a controlled child. Carries prompt, cadence, status, and history metadata.
- **Sub-tile** — the miniature representation of an assigned child that docks within an agent tile.

## End-to-End Workflow
1. User adds an application session; it lands in the dashboard and explorer as an unassigned child.
2. User adds an agent session; attach flow prompts for a short “job description”.
3. Dashboard synthesises the agent’s initial prompt from the user input + templated defaults and writes it to the agent terminal (followed by Enter).
4. User links one or more children to the agent via drag/drop (explorer or tile brain icon) or keyboard quick actions.
5. Private Beach notifies the agent runtime about the new assignment, including child metadata, permitted controls, and subscription cadence.
6. Agent acknowledges receipt; UI reflects the linkage (sub-tile appears, explorer tree updates).
7. While assigned, streaming updates flow from child → agent. The agent tile and relevant sub-tile pulse on each processed update.
8. User can inspect live output, toggle visibility, or open a history overlay summarising communications over time.

## Agent Onboarding & Prompt Generation
- **User input**: agent attach modal captures a concise free-form description of the agent’s responsibilities (1–3 sentences).
- **Template assembly**:
  - Merge user description into the default prompt scaffold that describes available MCP tools, `beach action` verbs, logging expectations, and update schema.
  - Pull real-time capability lists from backend (e.g., enabled actions, current resource URIs) so template stays current without user intervention.
  - Include structured format specification for streaming updates (JSON schema + examples) and command acknowledgement requirements.
- **Delivery**:
  - Write the assembled prompt into the agent terminal buffer, then send an Enter key event to trigger agent startup.
  - Persist the resolved prompt alongside the agent session metadata for replays and audit.
- **Visibility**:
  - Right detail pane (assignment editor) shows read-only copy of the “Prompt seed” plus a button to regenerate (with confirmation).
  - Explorer hover tooltips provide the first line of the prompt for quick scanning.

## Assignment Creation UX
- **Explorer drag & drop**:
  - Dragging an application node onto an agent node spawns the assignment creation flow.
  - Multi-select supported; dropping multiple apps onto one agent opens a batch confirmation with shared cadence defaults.
  - Keyboard fallback: `Enter` on application → “Assign to…” menu listing available agents.
- **Tile brain icon**:
  - Each application tile header exposes a draggable brain icon; dragging onto an agent tile creates the assignment.
  - When hovering over an agent tile, the tile shows a pulsing glow and “Drop to control” overlay.
  - Accessibility fallback: activate brain icon to open an action sheet listing agents.
- **Confirmation**:
  - After drop, slide-in assignment pane prepopulated with agent defaults; user can tweak cadence or scope before finalising.
  - On confirmation, we dispatch the assignment payload and await agent acknowledgement before marking UI as “Linked”.

## Assignment Messaging & Control Handover
- **Payload to agent** (draft structure):
  ```json
  {
    "type": "assignment.create",
    "childSessionId": "<uuid>",
    "childKind": "application|agent",
    "displayName": "App Alpha",
    "capabilities": {
      "actions": ["beach action run", "beach action stop"],
      "mcpTools": ["filesystem.read", "browser.navigate"]
    },
    "controlIntent": "Monitor deploy logs, restart on failure",
    "updateSchema": {
      "format": "json",
      "frequency": "5s",
      "fields": ["timestamp", "channel", "payload"]
    },
    "hints": {
      "preferredCadenceMs": 5000,
      "autoEscalateOn": ["error", "timeout"]
    }
  }
  ```
- **Acknowledgement**:
  - Agent responds with `assignment.ack` including a status (`accepted`, `deferred`, `rejected`) and optional notes.
  - UI applies optimistic state but flips to warning if no ack within timeout; explorer node shows “Pending…” badge.
- **Failure handling**:
  - If rejected, we surface the agent’s message and keep the child unassigned.
  - If ack times out, user can retry or force detach; logs capture the exchange for audit.

## Agent Tile & Sub-Tile Behaviour
- **Bottom bar**:
  - Once assigned, each child appears as a sub-tile tab in the agent tile’s footer, showing alias, status dot, and last update timestamp.
  - Clicking a sub-tile expands the child terminal inline; secondary click opens context menu (view history, detach, adjust cadence).
  - When numerous assignments exist, tabs collapse into a scrollable strip; overflow menu holds the remainder to prevent horizontal scroll bloat.
- **Hidden child tiles**:
  - Even if child tile is removed from the main canvas, the sub-tile remains accessible from the agent tile.
  - Hovering over a sub-tile previews the most recent log lines in a tooltip for quick inspection.
- **Status affordances**:
  - Transport health (connected, degraded, lost) displayed via icon tint and ARIA label.
  - Manual override indicator appears if a human has taken control away from the agent.

## Streaming Activity Feedback
- **Pulse effect**:
  - On each processed update, agent tile border and matching sub-tile emit a synchronized glow for 400 ms.
  - Cooldown prevents overlapping pulses; if updates arrive faster than cadence, glow intensifies but frame rate capped at 3 pulses/sec.
- **Log snippets**:
  - Agent tile header shows a rolling “last action” line (e.g., `Restarted beach-surfer (exit code 1)`).
  - Sub-tile tooltip includes the agent’s latest command directed at that child.
- **Mute controls**:
  - Users can temporarily mute visual pulses per sub-tile (for noisy streams); muted state badges the tab.

## History Inspection Flow
- **Entry points**:
  - Explorer: select agent then cmd/shift-click children, press `H` or use context menu “View history”.
  - Agent tile: select one or more sub-tiles (multi-select with modifier) and click “History” button in footer toolbar.
- **Overlay design**:
  - Full-screen (desktop) or sheet (mobile) overlay summarising events chronologically in tabular form (timestamp, direction, summary, payload link).
  - Filters for time range, child selection, severity (info / warning / error), and command category.
  - Supports breadcrumb navigation back to agent tile.
- **Data retention**:
  - Pulls from persisted assignment log store (minimum 24h retention). Display “history truncated” banner if retention window exceeded.
  - Provide export option (JSON download) for audits.
- **Accessibility**:
  - Overlay supports keyboard navigation with `Tab` order through filters, table rows, and close button.
  - Screen reader summary describes number of events and active filters.

## Backend & API Requirements
- Extend session attach endpoint to accept `role` and optional `jobDescription`.
- Prompt generator service composes templates, caches resolved prompts, and logs them for replay.
- Assignment creation API must fan out:
  - Persist assignment metadata (prompt ref, cadence, control scope).
  - Notify agent runtime via existing broker with payload + correlation ID.
  - Emit SSE/WebSocket updates so UI can render optimistic state and eventual ack.
- Expose deterministic seeding APIs so demo setup can be fully automated:
  - `GET /private-beaches/by-slug/:slug` **(new)** — resolve beach id by slug; returns 404 if absent.
  - `GET /private-beaches/:id/sessions` **(new)** — list sessions for a beach with filters (`alias`, `role`, `status`).
  - Extend `POST /private-beaches/:id/harness-bridge-token` to accept `alias`, desired `role`, and optional `labels` so seeds can predeclare intent.
  - `POST /private-beaches/:id/agents` **(new)** — bind an existing session id to an agent profile, generate the initial prompt from a supplied job description/template id, and inject it into the terminal.
  - `GET /private-beaches/:id/agents/:session_id` **(new, optional)** — fetch stored agent metadata (resolved prompt hash, last regenerated_at) for validation tooling.
  - `POST /private-beaches/:id/assignments` **(new)** — create one or more controller assignments in a single call; returns assignment ids and acknowledgement correlation tokens.
  - `GET /private-beaches/:id/assignments` **(new)** — list assignments for the beach with filters (`controller_session_id`, `child_session_id`, `status`).
  - `GET /private-beaches/:id/assignments/:assignment_id` **(new)** — query assignment status (`pending`, `accepted`, `rejected`) and surface agent-provided notes.
- Logging service upgrades to store agent-child message exchanges with indexing for history overlay.
- Telemetry events: assignment_created, assignment_ack, assignment_rejected, history_viewed, prompt_regenerated.

## Pong Showcase Seed Automation

### Objectives
- Allow `beach up` or Docker-based environments to mint a demo beach with one agent and two TUI Pong paddles in a single command.
- Keep the seed idempotent: re-running the script should reconcile existing sessions, prompts, and assignments without duplication.
- Exercise the same APIs the dashboard uses so the flow stays production-realistic and can serve as a regression harness.

### Seed Manifest
- Store manifests under `ops/seeds/private-beach/`. For Pong the canonical file is `ops/seeds/private-beach/pong-showcase.yaml`.
- Required keys:
  - `private_beach`: `{ slug, display_name, region }`.
  - `agent`: `{ alias, job_description, prompt_template_id, harness }`.
  - `sessions`: array of child sessions with `{ alias, harness, env, labels }`.
  - `assignments`: array mapping controller alias to child alias with `control_intent` and `update_frequency_ms`.
- Example:
  ```yaml
  version: 1
  private_beach:
    slug: pong-showcase
    display_name: Private Beach Pong Showcase
    region: us-west-2
  agent:
    alias: pong-manager
    job_description: >
      Coordinate the left and right Pong paddles, keep the match balanced,
      and restart players if their harness crashes.
    prompt_template_id: default-agent-v1
    harness:
      image: ghcr.io/beach/agent-runner:main
      command: ["beach", "agent", "run"]
      env:
        BEACH_AGENT_PROFILE: pong
  sessions:
    - alias: pong-left
      harness:
        image: ghcr.io/beach/pong-tui:main
        command: ["./bin/pong-left"]
      labels:
        role: paddle
        side: left
    - alias: pong-right
      harness:
        image: ghcr.io/beach/pong-tui:main
        command: ["./bin/pong-right"]
      labels:
        role: paddle
        side: right
  assignments:
    - controller: pong-manager
      children: ["pong-left", "pong-right"]
      control_intent: "Keep rally running; restart on crash."
      update_frequency_ms: 500
  ```
- Manifests double as documentation. CI can lint them via `yamllint` and a schema validator that matches the keys above.

### Seeder Script Flow (API Contract)
- Provide a Rust or Node CLI (`bin/seed-private-beach`) that ingests the manifest and performs the following steps. Each step calls an API that either already exists or is defined above:
  1. **Resolve beach (`GET /private-beaches/by-slug/:slug`, new).** If 404, create via existing `POST /private-beaches` with `{ slug, display_name, region }`. Capture resulting `private_beach_id`.
  2. **Fetch current sessions (`GET /private-beaches/:id/sessions`, new).** Used to decide whether harness containers already registered (match by alias label).
  3. **Mint bridge tokens (`POST /private-beaches/:id/harness-bridge-token`, extended).** Call once per manifest session (agent + paddles) supplying `{ alias, role, labels }`. Response returns `{ token, expires_at_ms, session_seed_id }`. Tokens pipe into container env (`BEACH_BRIDGE_TOKEN`).
  4. **Launch harness containers.** Script can emit an `.env` snippet consumed by `docker compose` (see below) or invoke `docker compose up --detach --wait`. Each container calls existing `POST /sessions/register` including `role` and `alias` from the manifest.
  5. **Poll for registration (`GET /private-beaches/:id/sessions`, new).** Wait until each alias reports `status=connected` and returns a concrete `session_id`.
  6. **Attach agent profile (`POST /private-beaches/:id/agents`, new).** Body `{ session_id, job_description, prompt_template_id }`. Manager generates the initial prompt, writes it to the agent terminal (with trailing Enter), and responds with `{ prompt_id, rendered_prompt, session_id }`.
  7. **Create assignments (`POST /private-beaches/:id/assignments`, new).** Provide `{ controller_session_id, children: [...], control_intent, update_frequency_ms }`. Response returns array of `{ assignment_id, status, ack_token }` (status is `pending` until agent acknowledges).
  8. **Confirm acknowledgement (`GET /private-beaches/:id/assignments/:assignment_id`, new).** Poll until status transitions to `accepted` (or handle `rejected` with surfaced reason). Script logs the final state.
  9. **(Optional) Set layout.** If layout API already exists, call `PUT /private-beaches/:id/layout` to apply stored tile geometry; otherwise skip until layout export/import is ready.
- CLI should expose `--dry-run` to print planned API calls without executing, and `--wait` to block until assignments are accepted.

### Docker / Compose Integration
- Provide `ops/seeds/private-beach/pong-showcase/docker-compose.yaml` with services `pong-manager`, `pong-left`, `pong-right`. Each service:
  - Uses the image declared in the manifest.
  - Accepts environment variables injected by the seeder (`BEACH_BRIDGE_TOKEN`, `PRIVATE_BEACH_ID`, `SESSION_ALIAS`).
  - Runs healthchecks that verify `POST /sessions/register` succeeded (e.g., poll `GET /private-beaches/:id/sessions` for its alias).
- The seeder writes `.seed/pong-showcase.env` containing the per-session tokens and passes it to `docker compose --env-file`.
- On shutdown the seeder can call `docker compose down` or leave containers running for demos; manifest includes `cleanup: true|false` flag to control behaviour.

### Validation & Telemetry
- After seeding, CLI performs a final validation pass:
  - Verify three sessions exist with expected roles via `GET /private-beaches/:id/sessions`.
  - Verify assignments accepted via `GET /private-beaches/:id/assignments?controller_session_id=<agent>` (extend endpoint to support query filter).
  - Ensure agent prompt stored by calling `GET /private-beaches/:id/agents/:session_id` (future read endpoint; optional).
- Emit telemetry events `seed_manifest_applied`, `seed_assignment_ack`, and `seed_failure` with manifest hash for traceability.
- Store seed run audit log under `s3://…/seeds/<slug>/<timestamp>.json` for reproducibility.

### Implementation Notes
- Manifest schema lives alongside TypeScript and Rust bindings so both CLI and services share validation.
- Service tokens used by the seeder require scopes `pb:private-beach.write`, `pb:session.write`, `pb:automation.write`.
- Provide an integration test that spins up Manager + Redis locally, runs the seeder in `--dry-run` and full modes, and asserts assignments reach `accepted`.
- Document the workflow in `docs/private-beach/pong-demo.md` with a “Quick Start” section linking back to this seed automation plan.

## Implementation Phasing
1. **Prompt platform**: build template service, attach flow integration, persistence.
2. **Assignment plumbing**: backend assignment API, agent acknowledgement protocol, SSE updates.
3. **UI scaffolding**: explorer drag/drop, brain icon interactions, assignment pane polish.
4. **Agent tile enhancements**: sub-tile footer, pulsing effect, overflow management.
5. **History overlay**: log store wiring, filtering UI, export flow.
6. **Hardening**: accessibility passes, telemetry validation, edge-case handling (timeouts, manual override).

## Edge Cases & Open Questions
- How do we represent hybrid sessions that can be both controller and controlled simultaneously? Need UI treatment before enabling.
- Should default cadence for new assignments derive from agent profile or child type? Might require per-agent preferences.
- What happens if agent disconnects mid-assignment? Decide whether children auto-detach or remain pending until reconnect.
- Are multiple agents allowed to control the same child concurrently? If yes, ensure updates route to all controllers without duplication.
- Determine rate limits for prompt regeneration and assignment retries to prevent agent thrashing.

---

This document should stay in sync with controller UX plans (`docs/private-beach/controller-agent-ux-plan.md`) and will serve as the blueprint for engineering breakdown tickets once open questions are resolved.

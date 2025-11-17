# Connection Timeline DevTools Expansion

Date: 2025-11-17

Owner: (Codex)

Status: Draft plan – ready for implementation by any agent

---

## Problem Statement

We currently expose high-level “fast-path” events in the rewrite-2 UI (initial connection, success/failure, fallback, etc.). That's helpful, but it still leaves on-call engineers blind to two critical flows:

1. **Viewer/browser ↔ manager ↔ host WebRTC status** – the browser fast-path timeline is lumped together with Beach Manager's controller logs. We can't see when the browser's peer connection fails, what triggered a reconnect, or how it correlates with the CLI host's view.
2. **Connector (agent→child) action flow** – when a tile-to-tile connector is created, it spins up another control channel (fast path, or HTTP fallback). We have no telemetry for those controller pipelines, so diagnosing “agent actions not applying” still requires digging through docker logs.

The goal is to turn the new Dev Tools panel into a comprehensive, per-session observability console covering both viewer WebRTC state and controller connectors, including action delivery telemetry.

---

## Goals

1. **Split timelines per session:** each session tile should display two collapsible tracks:
   - **Viewer ↔ Manager** – everything the browser knows about its WebRTC/WebSocket transport (ICE state, reconnects, fallback, etc.).
   - **Manager ↔ Host** – fast-path/HTTP events already logged by the CLI (manager hints, fast-path success, fallback, disconnect).
2. **Connector timelines:** every connector relationship (agent ↔ child) should get its own timeline summarizing:
   - Controller forwarder state (fast-path ready, fallback, reconnect, HTTP pollers).
   - Agent-originated actions, how the action was delivered (fast-path vs HTTP), and whether/why the child ACKed.
   - Updates flowing back from the child to the agent through the connector.
3. **Browser telemetry capture:** the rewrite-2 dashboard must emit WebRTC state changes (using the existing `TerminalViewerState` transport, RTC events, or a shim around `viewerConnectionService`) and send them through the devtools logging pipeline.
4. **Controller telemetry surface:** instrument the manager-side forwarder/CLI host to emit structured event payloads when controller channels change state or actions flow; expose those events to the dashboard (likely via Road/manager SSE or piggybacked on existing viewer transport).
5. **Copy-friendly logs:** console output should clearly identify which timeline (viewer, host, connector, action) an event belongs to, so engineers can copy/paste raw logs.

Non-goals:

- We are not building a full telemetry backend; events can stream through existing WS channels as simple JSON payloads.
- We will not replace existing CLI logs; this is an additive dev tool.

---

## Proposed Architecture

### 1. Viewer ↔ Manager Telemetry Capture

- Extend `viewerConnectionService` (apps/private-beach/src/controllers) to expose an event emitter whenever the underlying `RTCPeerConnection` changes `connectionState`, `iceConnectionState`, or `signalingState`.
- In rewrite-2's `useSessionConnection` hook, subscribe to these new events and log them separately from the manager fast-path entries. Use a new step prefix (`viewer.webrtc:*`) so the devtools store can distinguish them.
- Sample events:
  - `viewer.webrtc:connecting` (when `transport.status === 'connecting'`)
  - `viewer.webrtc:connected` (fast-path established, include ICE protocol, candidate type)
  - `viewer.webrtc:failed` (include browser error detail)
  - `viewer.webrtc:reconnecting` (trigger reason: ICE disconnect, track `reason`)

### 2. Manager ↔ Host Telemetry Augmentation

- The CLI already logs `fast-path controller channel ready`, `http action poller paused/resumed`, etc. Ensure the CLI host also emits structured JSON events (maybe via the existing `logConnectionEvent` CLI equivalent or a new channel) so rewrite-2 can render those without scraping docker logs.
- Option: write a small sidecar websocket that forwards CLI dev logs into Road for the dashboard. Alternatively, teach the manager to notify connected viewers about host transport state via the existing viewer WS channel.

### 3. Connector Timeline Capture

- Extend Beach Manager's controller forwarder to emit events whenever:
  - A controller (agent) is paired/unpaired with a child.
  - The agent fast-path data channel opens, closes, reconnects, or falls back to HTTP.
  - Actions are queued (include `actionId`, `transport=fast_path|http`, `controller_session`, `child_session`).
  - Child ACKs or rejects an action (include status/reason).
  - Child sends updates back via controller channel (emit a generic `controller.update` event without payload to avoid leakage).
- These events should be broadcast to the dashboard. Options:
  - Use the existing `controller.actions` SSE feed (if accessible to the UI) or add a new `/devtools/stream` endpoint the dashboard can subscribe to.
  - Alternatively, the rewrite UI can call a `GET /devtools/connection-log` endpoint to fetch recent events when the panel opens.

### 4. DevTools Store/Panel Enhancements

- Update `connectionTimelineStore` to support multiple tracks per session:
  - `session:<id>:viewer`
  - `session:<id>:host`
  - `connector:<controllerId>:<childId>`
- Provide grouping metadata in the store so the panel can render nested collapsibles: session header → viewer track, host track; connectors list with their own tracks.
- When recording connector events, include action metadata (action id, transport, outcome).
- Panel UI:
  - Each track should show the event stream from newest → oldest with filters and show/hide toggles.
  - Connector tracks should aggregate actions (maybe a mini table showing action id, send method, ack status, ack reason).
  - Make sure the console logging uses distinct prefixes (e.g., `[devtools][viewer-timeline]`, `[devtools][connector-timeline]`) to mirror the panel structure.

### 5. Action/Update Telemetry Source

- Instrument the CLI agent harness (`apps/private-beach/demo/pong/agent`, `crates/beach-buggy/src/fast_path.rs`) to log every action send + ack with structured metadata (tile id, connector id, transport, ack reason).
- Add a gRPC/HTTP emission from Beach Manager so the dashboard can receive these events:
  - When the agent host queues an action, manager can push an event to a `controller.actions.dev` SSE.
  - When the child host processes the action, manager/CLI should emit an `action.consume` event.
- Similarly, when a child sends `ControllerEvent::Update`, emit an `connector.update.sent` event (no payloads).

---

## Work Breakdown

1. **Design Telemetry Contracts (backend)**
   - Define event schemas for viewer WebRTC, host transport, connector action, and connector updates.
   - Document new SSE/Websocket endpoints or hooking strategy.

2. **Implement CLI/Manager Instrumentation**
   - Add event emitters in CLI host and agent harness.
   - Teach Beach Manager to relay connector events to interested dashboard clients.

3. **Extend rewrite-2 data layer**
   - Update `viewerConnectionService`/`useSessionConnection` to capture browser WebRTC events.
   - Create a devtools event subscription hook that consumes manager/connector SSE.
   - Update connection timeline store UI grouping.

4. **UI Enhancements**
   - Refactor DevTools panel into session sections with collapsible viewer/host tracks.
   - Add connectors section listing each agent-child pair and their action timeline.
   - Provide ability to toggle visibility, copy raw logs, etc.

5. **Telemetry Verification**
   - Run Pong demo with agent and ensure timelines capture:
     - Browser viewer fast-path connect/disconnect.
     - Host fast-path state changes.
     - Connector fast-path vs HTTP fallback transitions.
     - Agent actions, ACKs, and child updates.
   - Validate console logs match UI timelines.

6. **Documentation**
   - Update docs/private-beach/connection-issue-diagnoses with instructions on using the new panel.
   - Include instructions for enabling verbose logging and SSE endpoints locally.

---

## Copy-ready Prompt for Codex Implementation

```
You are picking up the "Connection Timeline DevTools Expansion" plan in docs/private-beach/connection-timeline-devtools-plan.md. Implement it end-to-end:

1. Instrument the browser viewer to log RTC connection state transitions (`viewerConnectionService` + rewrite hook).
2. Emit dedicated events for manager↔host transport state from the CLI/manager (add SSE or another feed the dashboard can consume).
3. Instrument connector controller flow (pairing, fast-path readiness, HTTP fallback, agent actions send/ack, child updates) so the UI can subscribe to those events.
4. Extend the rewrite devtools store/UI so each session shows separate “Viewer↔Manager” and “Manager↔Host” tracks, and each connector shows its own timeline with action/ack/update summaries.
5. Keep console logs in sync so engineers can copy the raw event stream.

Assume the backend transports can expose SSE/WS endpoints if needed; implement whatever plumbing is necessary to get the telemetry into the dashboard. Validate the timeline using the Pong demo (agent + players).
```

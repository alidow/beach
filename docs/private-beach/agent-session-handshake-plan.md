# Agent/Session Handshake Plan

_Last updated: 2025-11-11_

## Overview

Public Beach sessions (players, agents, viewers) should only know about **Beach Road** (port 4132). When an operator attaches one of those sessions to a Private Beach, **Beach Manager** must quietly establish its own controller channel without requiring new URLs or tokens from the session host. This document captures the architecture and implementation steps to reach that goal.

## Goals

1. **Public-facing simplicity** – hosts continue to run `beach host --session-server http://localhost:4132/` with no additional secrets.
2. **Private Beach control** – once attached, the manager drives controller leases, action fan-out, prompt packs, telemetry, etc.
3. **Fast-path preferred, HTTP fallback** – manager should use the existing WebRTC fast-path when possible, but fall back to HTTP/WebSocket delivery if needed.
4. **Traceability** – all controller traffic (actions, acks, transport status) continues to be logged and metered centrally.

## Current Pain

Today the only way to make an agent move the paddles is to rerun the hosts against `http://localhost:8080/`, which defeats the purpose of Beach Road. The agent queues `terminal_write` actions with the manager, but there is no return path from the manager back to sessions registered on Road, so the paddles stay static.

## Desired Flow

```
Public Host ──► Beach Road (register + transport)
          └─ attach to Private Beach ─► Beach Manager
                                          │
                                          ▼
                                Manager reaches back:
                                - Fast-path WebRTC data channel (preferred)
                                - HTTP/WebSocket fallback
                                          │
                                          ▼
                                   Controller actions
```

### Step-by-step

1. **Registration** – host registers with Beach Road, receives join code, and streams state via Road’s transport.
2. **Attachment** – operator uses the dashboard to attach the session to a Private Beach.
3. **Reach-back** – manager records the Road session metadata (Road session ID, signaling URL, ICE candidates). Manager initiates a controller channel back to that host:
   - **Fast-path**: manager uses the same offer/answer plumbing Road exposes to establish a WebRTC data channel directly with the host (manager = second peer).
   - **Fallback**: if fast-path fails, manager joins the session via Road’s HTTP/WebSocket interface to post commands.
4. **Action fan-out** – agent queues `terminal_write` on manager. Manager pushes them down the channel established above; acks and telemetry come back the same way.
5. **Redis bridge** – manager continues to write into `pb:<beach_id>:sess:<session_id>:actions`. Road (or a manager worker) subscribes to the stream with a dedicated consumer group per session. The first time a session is attached, the consumer group is created automatically (the change we just shipped guarantees this). Road drains entries, blasts them into the active transport, and posts acks back to the manager (which XACKs + updates metrics).
6. **UI + Prompts** – all prompt-pack logic stays in manager. Once the controller channel is live, manager can push prompt updates, emergency stops, etc., without the session knowing anything about the manager URL.

## Required Changes

### Beach Manager

- Persist the Road session metadata when a session is attached (Road session ID, signaling URL, ICE candidates, WebSocket URL).
- Expose a “reach-back” worker that:
  - Negotiates a fast-path data channel with the host (acting as offerer or answerer as needed).
  - If WebRTC fails, reuses Road’s HTTP/WebSocket transport to post actions.
  - Subscribes to `pb:<beach>:sess:<session>:actions` with a per-session consumer (already keyed per session); auto-create the consumer group when missing.
  - Sends controller actions over the chosen transport and collects acks/telemetry.
- Update metrics/logging so we can trace the manager→session delivery path (e.g., `controller.delivery` target already introduced).

### Beach Road

- Store the manager-facing stream key / metadata when a session registers (already included in the register response).
- Add a lightweight worker (one per attached session) that:
  - Subscribes to the manager’s action stream (via Redis or a manager HTTP endpoint).
  - Forwards payloads into the session’s transport (WebRTC/WebSocket).
  - Notifies the manager when actions are delivered (so the manager can XACK).
- Expose a small API to toggle the worker when a session is attached/detached.

### CLI Hosts

- No changes required. They keep pointing at Road (4132) and never need manager credentials or URLs.

## Implementation Steps

1. **Road storage & metadata**
   - Persist the manager stream key and signaling URLs in Road’s session record when `register_session` succeeds.
   - Include any data the manager will need for reach-back (ICE candidates, STUN/TURN hints).

2. **Manager reach-back worker**
   - On `attach_session`, create a record linking the Road session to the private beach.
   - Spawn a worker that:
     - Attempts fast-path negotiation (manager takes the “offerer” role if needed).
     - Falls back to Road’s HTTP/WebSocket client if WebRTC fails.
     - Subscribes to the Redis stream (ensuring the consumer group exists) and forwards actions.
     - Pushes acks/telemetry back to the manager APIs (so existing metrics remain accurate).

3. **Road action-forwarding worker**
   - When notified that a session is attached, start draining its manager action stream and emitting the commands over the session’s live transport.
   - When the session detaches, stop the worker to avoid orphaned consumers.

4. **Manager APIs for acks**
   - Either (a) expose a small HTTP endpoint Road can hit to XACK actions, or (b) let Road call the existing `/actions/ack`.
   - Update metrics (`FASTPATH_ACTIONS_SENT`, `QUEUE_DEPTH`, etc.) to include the Road-delivered channel.

5. **Fast-path handoff (bonus)**
   - Once the manager successfully negotiates its own WebRTC channel, Road can hand off controller delivery entirely (manager writes to the data channel directly, Road just keeps the transport alive for viewers/multiple peers).

6. **Telemetry & logging**
   - Expand the `controller.delivery` and `controller.actions` logs we just added so we can confirm the new workers are active.
   - Add Road-side logs (`road.controller.forwarder`) to trace each stage (subscribe, deliver, ack).

## Risks / Open Questions

- **Multiple controllers** – when the manager holds a reach-back channel, we must ensure the CLI host doesn’t treat it as a separate peer with conflicting input.
- **Resource usage** – per-session workers in Road/manager need bounding (avoid leaking tasks if sessions churn quickly).
- **Network topology** – managers running outside the same VPC may need TURN to reach sessions; ensure reach-back honors the same ICE config Road uses.

## Next Steps

1. Land redis consumer-group auto creation (already merged).
2. Prototype a Road worker that drains Redis and writes into a session’s WebRTC transport.
3. Add manager APIs to notify Road when a session is attached/detached (and vice versa for reach-back success/failure).
4. Iterate until the paddles move without hosts ever pointing at the manager URL.

Once this loop is closed, operators can keep launching public sessions against Beach Road, and the private beach experience “just works” when they attach a tile in the dashboard.


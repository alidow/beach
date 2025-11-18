# Connection Timeline DevTools Usage (Nov 17, 2025)

The rewrite dashboard now exposes full-fidelity telemetry for every viewer session and controller connector. Use this when validating Pong or triaging fast-path regressions.

## Session timelines

- Open the **Dev Tools** drawer and you will see one card per session. Each card now renders two tracks:
  - **Viewer ↔ Manager** – populated by the browser (`useSessionConnection`) and the WebRTC diagnostics shim. Every RTC connection, ICE, and signaling transition lands here with `viewer.webrtc:*` steps.
  - **Manager ↔ Host** – streamed directly from Beach Manager via `/sessions/:id/devtools/stream`. Fast-path upgrades, ack stalls, fallbacks, cancellations, etc. show up as `host.transport:*`.
- Console output mirrors the panel (`[beach-connection] host.transport:fallback ...`) so you can copy/paste raw sequences into incident docs.

## Connector timelines

- Each controller↔child pairing gets its own card under **Connectors**. Track entries include:
  - `connector.action:queued|forwarded|ack|rejected` with transport metadata (`fast_path` vs `http_fallback`, seq ids, latency).
  - `connector.transport:update` when a connector graduates to fast-path or drops to HTTP.
  - `connector.child:update` pulses whenever the child emits viewer diffs back through the connector.
- Events are delivered through the same SSE feed, so the panel updates in near real time while running the Pong demo.

## Validating with Pong

1. Start the demo (`apps/private-beach/demo/pong/tools/run-agent.sh`) with at least one player and the agent.
2. Open the Dev Tools drawer on the rewrite dashboard.
3. Ensure:
   - Viewer track logs WebRTC state changes (`connected → disconnected → reconnecting`) when you toggle network conditions in browsers.
   - Host track reports `host.transport:fast-path-ready` once the CLI completes fast-path enrollment, and logs `ack-stall` or `fallback` when you pause the agent.
   - Connector cards show queued actions when the agent sends keystrokes and `ack` entries with the right latency once children apply them.
4. If anything fails to appear, tail `apps/beach-manager` logs for `[devtools][timeline]` statements; the same payloads should hit the SSE stream.

Refer engineers here during on-call handoffs so they can rely on the dashboard before diving into Docker logs.

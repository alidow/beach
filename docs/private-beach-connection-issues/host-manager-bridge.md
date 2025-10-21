# Private Beach “Connecting…” Stall

## Current Behavior
- After attaching a public session in the Private Beach UI, the tile remains stuck on “Connecting…”.
- Beach Manager mints a bridge token and calls Beach Road `/sessions/:id/join-manager`, which logs `join-manager request received; dispatching bridge hint`.
- Beach Road tries to deliver the hint, but because no harness peer of role `Server` is subscribed, it queues the message (`queued join-manager hint until harness reconnects`).
- The host CLI never registers the session with the manager, and the SSE stream seen by the UI stays empty, so no terminal frames render.

## Root Cause
- The host CLI only subscribes to manager bridge hints when a WebRTC **offerer** transport is negotiated.
- In the Private Beach flow there is no WebRTC viewer; the host becomes the **answerer** (or falls back to a single transport), so the subscription path is never executed.
- Without that subscription, the bridge hint queue is never drained, the harness never calls `POST /sessions/register`, and the UI cannot progress past “Connecting…”.

## Plan of Record
1. **Surface the signaling client for all WebRTC roles**  
   - Extend the WebRTC negotiation result so the host retains access to the `SignalingClient` even when acting as an answerer.  
   - Include that handle in `NegotiatedSingle` so callers can subscribe to manager hints regardless of transport flavor.
2. **Subscribe unconditionally in the host**  
   - When any negotiated transport reports a signaling client, spawn the manager hint listener immediately (guarded by the existing atomic).  
   - Keep the listener logic shared so offerer and answerer paths behave identically.
3. **Verify harness registration loop**  
   - Ensure the listener feeds the existing harness setup (register + terminal frame pump).  
   - Capture logs to confirm the manager receives registration and terminal diffs without manual `curl`s.

## Reality Check
- Once the host is subscribed, Beach Road’s queued hints will be delivered the moment negotiation completes, regardless of WebRTC role.  
- The existing harness machinery already turns hints into `register_session` calls; the only missing link was the subscription.  
- By exposing the `SignalingClient` and subscribing for every transport, we eliminate the gap observed in the console logs, so the UI should progress to live terminal output after attachment.

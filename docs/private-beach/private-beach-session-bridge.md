# Private Beach Session Bridge Fix

## Problem Statement
- Private Beach tiles stayed in “Connecting…” after attaching a public session.
- Manager minted bridge tokens and nudged Beach Road, but the host CLI never registered the session or streamed terminal state.
- Beach Road’s `/sessions/:id/join-manager` handler was a no-op; the CLI had no mechanism to receive bridge hints.
- Without a registered harness, the manager’s SSE stream remained idle and the UI never advanced.

## Observed Symptoms
- Manager logs: repeated `session stream subscribed` but no `register_session invoked`.
- Beach Road logs: join-manager request logged, yet no downstream activity from the host.
- Browser console: repeated SSE connection closures on the terminal stream.
- CLI logs: only “session registered” and “host ready”, no evidence of manager communication.

## Fix Overview
1. **Manager → Road payload**  
   Include `private_beach_id` when nudging Beach Road so downstream components know which private beach is requesting the bridge.

2. **Beach Road hint delivery**  
   - Convert `/sessions/:id/join-manager` into a real bridge hint dispatcher.  
   - Broadcast hints to connected harness peers and persist them if the harness is offline.  
   - Replay queued hints the moment the harness reconnects.

3. **WebSocket signaling client**  
   - Extend the host’s WebRTC signaling client to surface `ManagerBridgeHint` messages via a new async channel.
   - Track manager-hint notifications so the host knows when to initiate registration.

4. **CLI host integration**  
   - Subscribe to the manager-hint channel when the host becomes the WebRTC offerer.
   - Spin up a `beach-buggy` `SessionHarness` configured with the manager URL + bridge token from the hint.
   - Auto-register with the manager, mark state as dirty, and periodically push terminal frames captured from the local PTY grid.
   - Persist dirty state and cursor updates so subsequent diffs flow without user intervention.

5. **Logging & Diagnostics**  
   - Added information-level tracing on the manager nudge and Beach Road hint dispatch.  
   - CLI now logs when the harness subscription is established and when manager hints arrive, making end-to-end traces easier to follow.

## Result
- Attaching a public session now triggers an automatic harness registration and terminal streaming loop.
- Private Beach UI transitions from “Connecting…” to rendering terminal output without manual `curl` workarounds.
- If the harness reconnects after being offline, queued hints are replayed, ensuring resumed connectivity.

# Beach Surfer Connection Hang

## What We Observed
- The Beach Surfer web client establishes the WebRTC link, completes the Noise handshake, and receives streaming snapshot frames, yet the UI stays on “Waiting for host approval”.
- Terminal input never reaches the host, and the connection appears stuck even though the underlying transport is healthy.
- Console traces (see `temp/beach-surfer.log`) show `frame snapshot`, `frame heartbeat`, etc., but there is no `frame hello` entry.

## Root Cause
- When the viewer auto-connects, `BeachSessionView` sniffs the very first binary payload from the data channel and queues it through `replayBinaryFirst` when instantiating `DataChannelTerminalTransport`.
- The transport constructor immediately decodes and dispatches that cached frame before React has mounted `BeachTerminal` and attached its `frame` event listener.
- The first payload is the required `HostFrame::Hello`. Because it is dispatched before any listeners exist, the hello is dropped.
- Without the hello:
  - `subscriptionRef` remains `null`, preventing outbound input.
  - `handshakeReadyRef` never flips to true, so the join overlay keeps showing “waiting…”.

## Proposed Fix
- Defer the replay of `replayBinaryFirst` until after listeners can subscribe—e.g., wrap the dispatch in `queueMicrotask`, `Promise.resolve().then(...)`, or `setTimeout(..., 0)`.
- This ensures `BeachTerminal`’s effect runs, registers the `frame` handler, and then receives the cached hello frame, restoring the normal approval → snapshot flow and enabling input.

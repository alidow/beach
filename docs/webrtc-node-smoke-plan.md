# WebRTC Node Smoke Test Plan (Manager + Browser Paths)

Goal: add a headless, Node-based smoke test that proves both manager↔host and browser↔host WebRTC can negotiate and exchange framed messages against the real Beach Road signaling. The test should run from the host (Node), with Road/Manager in Docker.

## Prereqs
- `direnv allow` in repo root; ensure `.envrc` resolves ICE to a host-reachable IP (`host.docker.internal` preferred).
- Docker stack up with Road/Manager built:
  ```
  direnv exec . sh -c 'BEACH_SESSION_SERVER=http://beach-road:4132 \
    PONG_WATCHDOG_INTERVAL=10.0 \
    BEACH_MANAGER_STDOUT_LOG=trace BEACH_MANAGER_FILE_LOG=trace BEACH_MANAGER_TRACE_DEPS=1 \
    RUST_LOG="beach::transport::webrtc=trace,beach::transport::webrtc::signaling=trace,beach::session=debug" \
    DEV_ALLOW_INSECURE_MANAGER_TOKEN=1 DEV_MANAGER_INSECURE_TOKEN=DEV-MANAGER-TOKEN \
    PRIVATE_BEACH_MANAGER_TOKEN=DEV-MANAGER-TOKEN PRIVATE_BEACH_BYPASS_AUTH=0 \
    ./scripts/dockerdown --postgres-only && docker compose down && docker compose build beach-manager && docker compose up -d'
  ```
- Host session id + passcode (e.g., from `beach create` or the pong-stack bootstrap output).

## Signaling model to replicate (matches browser)
1) WebSocket connect to `ws://beach-road:4132/ws/:session_id`.
2) Send `ClientMessage::Join` with:
   - `peer_id` (generate UUID)
   - `passphrase` = host passcode
   - `supported_transports` = [`webrtc`], `preferred_transport` = `webrtc`
   - optional `label` (`beach-manager` or `private-beach-dashboard`)
3) On `JoinSuccess`, collect `peer_id` (self) and peer list.
4) Discover host peer_id:
   - Call new debug endpoint `GET /debug/sessions/:host_session_id/peers` (returns `PeerInfo[]`); pick the entry with `role=server`.
   - Fallback: watch `PeerJoined` events until a `role=server` appears.
5) Negotiate:
   - Send `NegotiateTransport` to host peer_id proposing `webrtc`.
   - Expect `TransportProposal`/`TransportAccepted` back (mirror browser logic: accept same transport).
6) WebRTC signaling over `Signal` messages:
   - For offerer role (peer with lower lexical peer_id, to keep it deterministic):
     - Create RTCPeerConnection (werift), create data channel label `beach`, gather ICE.
     - Send `Signal` with `{ signal_type:"sdp_offer", sdp, type, handshake_id }` to host.
     - Send ICE candidates as `Signal { signal_type:"ice_candidate", ... }`.
   - For answerer role:
     - Wait for `Signal` offer, setRemoteDescription, create answer, send `Signal answer`.
     - Add incoming ICE candidates.
7) Data channel:
   - Wait for `beach` channel `open` on both sides.
   - Exchange framed ping/ack using the framing from `run.js` (namespace `controller`, kinds `input`/`ack`, CRC+optional HMAC).

## Implementation tasks
1) **Road debug endpoint (done)**: `GET /debug/sessions/:id/peers` returns live peer info (id/role/metadata) from websocket state.
2) **Node harness** (add `apps/beach/tests/node-webrtc/smoke.js`):
   - Dependencies: `ws`, `werift`, reuse framing helpers from `run.js`.
   - Helpers:
     - `connectSignaling(sessionId, passcode, label)` → returns `{ws, peerId, peers, send(msg), onMessage(cb), close()}`.
     - `fetchHostPeerId(hostSessionId)` via debug endpoint.
     - `negotiateWebRtc(localPeerId, hostPeerId, ws, role)` to run offer/answer/ICE over `Signal`.
     - `openDataChannel(pc, label)` to wait for “beach” channel.
     - `exchangeFrames(channel)` to send framed ping and expect ack.
   - Two test cases:
     - Viewer-like: label `private-beach-dashboard`, passcode = host passcode.
     - Manager-like: label `beach-manager`, passcode = controller passcode (can reuse host passcode if same).
   - Exit nonzero on any failure; log which leg failed.
   - Config via env:
     - `ROAD_URL` (default `http://localhost:4132`)
     - `HOST_SESSION_ID`, `HOST_PASSCODE` (required)
     - `CONTROLLER_PASSCODE` (optional; default to host passcode)
3) **package.json**: add script `"smoke": "node smoke.js"` in `apps/beach/tests/node-webrtc`.
4) **Docs**: note the required envs and the command to run:
   ```
   ROAD_URL=http://localhost:4132 \
   HOST_SESSION_ID=<id> \
   HOST_PASSCODE=<code> \
   CONTROLLER_PASSCODE=<code> \
   npm --prefix apps/beach/tests/node-webrtc run smoke
   ```

## Prompt for implementation (for a fresh Codex instance)
```
You are in the beach repo. Implement a Node WebRTC smoke test using the Road websocket signaling:
- There is a debug endpoint GET /debug/sessions/:id/peers returning live PeerInfo (id/role/metadata) from websocket state.
- Add apps/beach/tests/node-webrtc/smoke.js that:
  * Connects to ws://beach-road:4132/ws/:session_id (config ROAD_URL env).
  * Sends Join with passcode, supports/preferred transport webrtc, label (configurable).
  * Fetches host peer_id via the debug endpoint; if missing, waits for PeerJoined with role=server.
  * Runs transport negotiation (NegotiateTransport/AcceptTransport) for webrtc.
  * Performs offer/answer + ICE over Signal messages using werift RTCPeerConnection; use “beach” data channel.
  * Sends a framed ping (reuse encodeFrame/decodeFrame from run.js) and expects an ack frame back.
  * Runs two cases: viewer-like (label private-beach-dashboard, pass HOST_PASSCODE) and manager-like (label beach-manager, pass CONTROLLER_PASSCODE or HOST_PASSCODE).
  * Exits nonzero on any failure; logs per-leg success.
- Update apps/beach/tests/node-webrtc/package.json scripts to add "smoke": "node smoke.js".
- Do not change other code. Keep test config via env: ROAD_URL (default http://localhost:4132), HOST_SESSION_ID, HOST_PASSCODE, CONTROLLER_PASSCODE (optional).
After coding, provide run instructions.
```

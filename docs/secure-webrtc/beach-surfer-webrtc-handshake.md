# Beach Surfer WebRTC Handshake Regression

_Last updated: 2025-09-28_

## Repro setup
- Host: `cargo run -- --session-server http://127.0.0.1:8080 --log-file ~/beach-debug/host.log`
- Signaling (beach-road): `cargo run`
- Browser client: `pnpm --filter beach-surfer dev` (open the advertised session URL in http://localhost:5173)

## What used to go wrong
- Signaling completed (offer/answer exchange, ICE trickle), but the browser never rendered the host prompt.
- `beach-road` logs showed the client polling `/webrtc/answer` until it appeared; signaling itself was fine.
- The host immediately attempted to stream Hello/Grid/Snapshot over WebRTC and hit `DataChannel is not opened`.
- Because the forwarder kept the stale transport alive, every retry spoke into the same closed channel. The terminal remained blank.

### Root cause (pre-fix)
The browser sent the `"__ready__"` sentinel from the RTCPeerConnection `onopen` handler **before** any consumer attached a `message` listener to the `RTCDataChannel`. The host interpreted that sentinel as “safe to send snapshots” and flushed the initial frames right away. Those frames arrived before the front-end wired up `DataChannelTerminalTransport`, so the browser dropped them on the floor. The host kept retrying, hit the `DataChannel is not opened` error, and the join never recovered.

## Fix implemented on 2025-09-28
- `WebRtcTransport` now exposes an `isOpen()` helper so higher-level code can detect when the channel is actually ready.
- `DataChannelTerminalTransport` sends the `"__ready__"` sentinel only after it has installed its message listeners. If the data channel opens later, it waits for the `open` event before sending.
- Added regression tests (`apps/beach-surfer/src/transport/terminalTransport.test.ts`) to cover both the “already open” and “open later” paths, ensuring the sentinel fires exactly once.

This guarantees the host will not send Hello/Grid/Snapshot until the browser is prepared to receive them.

## What to verify next
1. Re-run the end-to-end session and confirm the browser now displays the initial prompt within a few seconds.
2. Watch `~/beach-debug/host.log` to ensure the `DataChannel is not opened` and "offerer did not receive readiness ack" warnings no longer appear for new sessions.
3. If any transports still fail to come up, capture fresh host/browser logs so we can inspect the new failure mode (we should no longer see dropped snapshots due to missing listeners).

Keeping this note up to date should save the next pass from rediscovering the readiness-ordering pitfall.


---

LATEST:

still the same issue after your last fixes. pls diagnose, check logs, etc

beach-road: ```(base) arellidow@Arels-MacBook-Pro beach-road % cargo run
   Compiling beach-road v0.1.0 (/Users/arellidow/development/beach/apps/beach-road)
warning: unused import: `info`
 --> apps/beach-road/src/cli.rs:7:29
  |
7 | use tracing::{debug, error, info};
  |                             ^^^^
  |
  = note: `#[warn(unused_imports)]` on by default

warning: unused import: `info`
  --> apps/beach-road/src/handlers.rs:11:29
   |
11 | use tracing::{debug, error, info};
   |                             ^^^^

warning: unused import: `trace`
  --> apps/beach-road/src/websocket.rs:13:35
   |
13 | use tracing::{debug, error, info, trace, warn};
   |                                   ^^^^^

warning: unused variable: `from_peer`
   --> apps/beach-road/src/cli.rs:203:49
    |
203 |                         ServerMessage::Signal { from_peer, signal } => {
    |                                                 ^^^^^^^^^ help: try ignoring the field: `from_peer: _`
    |
    = note: `#[warn(unused_variables)]` on by default

warning: variant `Ipc` is never constructed
  --> apps/beach-road/src/handlers.rs:25:5
   |
22 | pub enum AdvertisedTransportKind {
   |          ----------------------- variant in this enum
...
25 |     Ipc,
   |     ^^^
   |
   = note: `AdvertisedTransportKind` has derived impls for the traits `Clone` and `Debug`, but these are intentionally ignored during dead code analysis
   = note: `#[warn(dead_code)]` on by default

warning: function `generate_session_id` is never used
 --> apps/beach-road/src/session.rs:5:8
  |
5 | pub fn generate_session_id() -> String {
  |        ^^^^^^^^^^^^^^^^^^^

warning: methods `delete_session` and `clear_webrtc_offer` are never used
   --> apps/beach-road/src/storage.rs:78:18
    |
39  | impl Storage {
    | ------------ methods in this implementation
...
78  |     pub async fn delete_session(&mut self, session_id: &str) -> Result<()> {
    |                  ^^^^^^^^^^^^^^
...
106 |     pub async fn clear_webrtc_offer(&mut self, session_id: &str) -> Result<()> {
    |                  ^^^^^^^^^^^^^^^^^^

warning: field `session_id` is never read
  --> apps/beach-road/src/websocket.rs:24:5
   |
22 | struct PeerConnection {
   |        -------------- field in this struct
23 |     peer_id: String,
24 |     session_id: String,
   |     ^^^^^^^^^^
   |
   = note: `PeerConnection` has a derived impl for the trait `Clone`, but this is intentionally ignored during dead code analysis

warning: field `storage` is never read
  --> apps/beach-road/src/websocket.rs:38:5
   |
34 | pub struct SignalingState {
   |            -------------- field in this struct
...
38 |     storage: SharedStorage,
   |     ^^^^^^^
   |
   = note: `SignalingState` has a derived impl for the trait `Clone`, but this is intentionally ignored during dead code analysis

warning: `beach-road` (bin "beach-road") generated 9 warnings (run `cargo fix --bin "beach-road"` to apply 3 suggestions)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.21s
     Running `/Users/arellidow/development/beach/target/debug/beach-road`
2025-09-28T22:48:12.218051Z  INFO beach_road: Starting Beach Road session server on port 8080
2025-09-28T22:48:12.218195Z  INFO beach_road: Redis URL: redis://localhost:6379
2025-09-28T22:48:12.218230Z  INFO beach_road: Session TTL: 3600 seconds
2025-09-28T22:48:12.224914Z  INFO beach_road: Beach Road listening on 0.0.0.0:8080
🏖️  Beach Road listening on 0.0.0.0:8080
2025-09-28T22:48:24.961413Z DEBUG request{method=POST uri=/sessions version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:24.961662Z DEBUG request{method=POST uri=/sessions version=HTTP/1.1}: beach_road::handlers: Registering session: 0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038
2025-09-28T22:48:24.967510Z DEBUG request{method=POST uri=/sessions version=HTTP/1.1}: beach_road::handlers: Session 0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038 registered successfully
2025-09-28T22:48:24.967594Z DEBUG request{method=POST uri=/sessions version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=200
2025-09-28T22:48:24.978219Z DEBUG request{method=GET uri=/ws/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038 version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:24.978347Z DEBUG request{method=GET uri=/ws/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038 version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=0 ms status=101
2025-09-28T22:48:24.978477Z DEBUG beach_road::websocket: WebSocket connected: peer=8dd3939f-bae1-48d8-82ff-6adc21787b51 session=0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038
2025-09-28T22:48:24.979027Z DEBUG beach_road::websocket: Received WebSocket frame from peer 8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:24.979050Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:24.979058Z DEBUG beach_road::websocket: Text frame content from 8dd3939f-bae1-48d8-82ff-6adc21787b51: {"type":"join","peer_id":"387c41b6-6f50-4e71-a014-116eccb3543d","passphrase":"406244","supported_transports":["webrtc"],"preferred_transport":"webrtc"}
2025-09-28T22:48:24.979272Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Join { peer_id: "387c41b6-6f50-4e71-a014-116eccb3543d", passphrase: Some("406244"), supported_transports: [WebRTC], preferred_transport: Some(WebRTC) }
2025-09-28T22:48:24.979290Z  INFO beach_road::websocket: 📥 RECEIVED Join message from peer 8dd3939f-bae1-48d8-82ff-6adc21787b51 (client_peer_id: "387c41b6-6f50-4e71-a014-116eccb3543d") for session 0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038
2025-09-28T22:48:24.979302Z  INFO beach_road::websocket:   → First peer in session, assigning role: Server
2025-09-28T22:48:24.979324Z  INFO beach_road::websocket:   → Added peer 8dd3939f-bae1-48d8-82ff-6adc21787b51 to session 0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038 with role Server
2025-09-28T22:48:24.979407Z  INFO beach_road::websocket:   → Session now has 1 peers, available transports: [WebRTC]
2025-09-28T22:48:24.979433Z  INFO beach_road::websocket: 📤 SENDING JoinSuccess to peer 8dd3939f-bae1-48d8-82ff-6adc21787b51: session=0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038, peer_id=8dd3939f-bae1-48d8-82ff-6adc21787b51, peers=1, transports=[WebRTC]
2025-09-28T22:48:24.979444Z  INFO beach_road::websocket:   → JoinSuccess sent successfully to peer 8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:24.980178Z DEBUG beach_road::websocket: Received WebSocket frame from peer 8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:24.980196Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:24.980205Z DEBUG beach_road::websocket: Text frame content from 8dd3939f-bae1-48d8-82ff-6adc21787b51: {"type":"ping"}
2025-09-28T22:48:24.980219Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Ping
2025-09-28T22:48:24.982999Z DEBUG request{method=POST uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/offer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:24.987198Z DEBUG request{method=POST uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/offer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=204
2025-09-28T22:48:24.987704Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:24.988286Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=0 ms status=404
2025-09-28T22:48:25.241360Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:25.242343Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=404
2025-09-28T22:48:25.495062Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:25.495954Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=0 ms status=404
2025-09-28T22:48:25.748219Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:25.750043Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=404
2025-09-28T22:48:26.001800Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:26.003277Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=404
2025-09-28T22:48:26.264556Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:26.270274Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-28T22:48:26.527063Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:26.529041Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-28T22:48:26.784136Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:26.793737Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=9 ms status=404
2025-09-28T22:48:27.062055Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:27.064165Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-28T22:48:27.320593Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:27.337274Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=16 ms status=404
2025-09-28T22:48:27.592915Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:27.595217Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-28T22:48:27.852856Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:27.856422Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-28T22:48:28.112763Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:28.120456Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-28T22:48:28.374909Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:28.378913Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-28T22:48:28.637289Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:28.641191Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-28T22:48:28.896115Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:28.904987Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=9 ms status=404
2025-09-28T22:48:29.161770Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:29.164932Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-28T22:48:29.419102Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:29.424868Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-28T22:48:29.680345Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:29.686785Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-28T22:48:29.942671Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:29.943773Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=404
2025-09-28T22:48:30.199214Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:30.204865Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-28T22:48:30.459722Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:30.464074Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-28T22:48:30.717332Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:30.719504Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-28T22:48:30.972360Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:30.975083Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-28T22:48:31.233311Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:31.237007Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-28T22:48:31.491369Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:31.492670Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=404
2025-09-28T22:48:31.745000Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:31.749833Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-28T22:48:32.002785Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:32.005926Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-28T22:48:32.260155Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:32.262343Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-28T22:48:32.290324Z DEBUG request{method=POST uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/join version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:32.290455Z DEBUG request{method=POST uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/join version=HTTP/1.1}: beach_road::handlers: Client attempting to join session: 0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038
2025-09-28T22:48:32.292060Z DEBUG request{method=POST uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/join version=HTTP/1.1}: beach_road::handlers: Client successfully joined session: 0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038
2025-09-28T22:48:32.292183Z DEBUG request{method=POST uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/join version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=200
2025-09-28T22:48:32.296891Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/offer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:32.299029Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/offer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=200
2025-09-28T22:48:32.305933Z DEBUG request{method=GET uri=/ws/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038 version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:32.306032Z DEBUG request{method=GET uri=/ws/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038 version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=0 ms status=101
2025-09-28T22:48:32.306128Z DEBUG beach_road::websocket: WebSocket connected: peer=b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3 session=0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038
2025-09-28T22:48:32.307037Z DEBUG beach_road::websocket: Received WebSocket frame from peer b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3
2025-09-28T22:48:32.307049Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3
2025-09-28T22:48:32.307057Z DEBUG beach_road::websocket: Text frame content from b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3: {"type":"join","peer_id":"7f6c0377-4892-4fa7-baf4-6ef77698d9c0","passphrase":"406244","supported_transports":["webrtc"],"preferred_transport":"webrtc"}
2025-09-28T22:48:32.307095Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Join { peer_id: "7f6c0377-4892-4fa7-baf4-6ef77698d9c0", passphrase: Some("406244"), supported_transports: [WebRTC], preferred_transport: Some(WebRTC) }
2025-09-28T22:48:32.307112Z  INFO beach_road::websocket: 📥 RECEIVED Join message from peer b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3 (client_peer_id: "7f6c0377-4892-4fa7-baf4-6ef77698d9c0") for session 0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038
2025-09-28T22:48:32.307157Z  INFO beach_road::websocket:   → Session has existing peers, assigning role: Client
2025-09-28T22:48:32.307172Z  INFO beach_road::websocket:   → Added peer b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3 to session 0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038 with role Client
2025-09-28T22:48:32.307221Z  INFO beach_road::websocket:   → Session now has 2 peers, available transports: [WebRTC]
2025-09-28T22:48:32.307249Z  INFO beach_road::websocket: 📤 SENDING JoinSuccess to peer b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3: session=0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038, peer_id=b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3, peers=2, transports=[WebRTC]
2025-09-28T22:48:32.307264Z  INFO beach_road::websocket:   → JoinSuccess sent successfully to peer b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3
2025-09-28T22:48:32.307834Z DEBUG beach_road::websocket: Received WebSocket frame from peer 8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:32.307854Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:32.307862Z DEBUG beach_road::websocket: Text frame content from 8dd3939f-bae1-48d8-82ff-6adc21787b51: {"type":"signal","to_peer":"b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3","signal":{"signal":{"candidate":"candidate:3173101093 1 udp 2130706431 192.168.68.52 59494 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-28T22:48:32.307917Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3", signal: Object {"signal": Object {"candidate": String("candidate:3173101093 1 udp 2130706431 192.168.68.52 59494 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-28T22:48:32.307962Z DEBUG beach_road::websocket: Received Signal from 8dd3939f-bae1-48d8-82ff-6adc21787b51 to b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3: Object {"signal": Object {"candidate": String("candidate:3173101093 1 udp 2130706431 192.168.68.52 59494 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-28T22:48:32.308018Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=8dd3939f-bae1-48d8-82ff-6adc21787b51 to=b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3
2025-09-28T22:48:32.308051Z DEBUG beach_road::websocket: Received WebSocket frame from peer 8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:32.308059Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:32.308067Z DEBUG beach_road::websocket: Text frame content from 8dd3939f-bae1-48d8-82ff-6adc21787b51: {"type":"signal","to_peer":"b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3","signal":{"signal":{"candidate":"candidate:2193730423 1 udp 2130706431 100.88.228.18 59377 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-28T22:48:32.308094Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3", signal: Object {"signal": Object {"candidate": String("candidate:2193730423 1 udp 2130706431 100.88.228.18 59377 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-28T22:48:32.308112Z DEBUG beach_road::websocket: Received Signal from 8dd3939f-bae1-48d8-82ff-6adc21787b51 to b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3: Object {"signal": Object {"candidate": String("candidate:2193730423 1 udp 2130706431 100.88.228.18 59377 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-28T22:48:32.308138Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=8dd3939f-bae1-48d8-82ff-6adc21787b51 to=b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3
2025-09-28T22:48:32.308173Z DEBUG beach_road::websocket: Received WebSocket frame from peer 8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:32.308180Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:32.308186Z DEBUG beach_road::websocket: Text frame content from 8dd3939f-bae1-48d8-82ff-6adc21787b51: {"type":"signal","to_peer":"b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3","signal":{"signal":{"candidate":"candidate:2839139417 1 udp 2130706431 fd7a:115c:a1e0::3601:e412 59413 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-28T22:48:32.308212Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3", signal: Object {"signal": Object {"candidate": String("candidate:2839139417 1 udp 2130706431 fd7a:115c:a1e0::3601:e412 59413 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-28T22:48:32.308231Z DEBUG beach_road::websocket: Received Signal from 8dd3939f-bae1-48d8-82ff-6adc21787b51 to b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3: Object {"signal": Object {"candidate": String("candidate:2839139417 1 udp 2130706431 fd7a:115c:a1e0::3601:e412 59413 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-28T22:48:32.308240Z DEBUG beach_road::websocket: Received WebSocket frame from peer b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3
2025-09-28T22:48:32.308253Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3
2025-09-28T22:48:32.308256Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=8dd3939f-bae1-48d8-82ff-6adc21787b51 to=b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3
2025-09-28T22:48:32.308262Z DEBUG beach_road::websocket: Text frame content from b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3: {"type":"ping"}
2025-09-28T22:48:32.308277Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Ping
2025-09-28T22:48:32.308279Z DEBUG beach_road::websocket: Received WebSocket frame from peer 8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:32.308287Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:32.308293Z DEBUG beach_road::websocket: Text frame content from 8dd3939f-bae1-48d8-82ff-6adc21787b51: {"type":"signal","to_peer":"b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3","signal":{"signal":{"candidate":"candidate:2323497841 1 udp 1694498815 72.227.131.114 51625 typ srflx raddr 0.0.0.0 rport 51625","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-28T22:48:32.308323Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3", signal: Object {"signal": Object {"candidate": String("candidate:2323497841 1 udp 1694498815 72.227.131.114 51625 typ srflx raddr 0.0.0.0 rport 51625"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-28T22:48:32.308342Z DEBUG beach_road::websocket: Received Signal from 8dd3939f-bae1-48d8-82ff-6adc21787b51 to b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3: Object {"signal": Object {"candidate": String("candidate:2323497841 1 udp 1694498815 72.227.131.114 51625 typ srflx raddr 0.0.0.0 rport 51625"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-28T22:48:32.308366Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=8dd3939f-bae1-48d8-82ff-6adc21787b51 to=b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3
2025-09-28T22:48:32.311327Z DEBUG request{method=POST uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:32.311539Z DEBUG beach_road::websocket: Received WebSocket frame from peer b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3
2025-09-28T22:48:32.311554Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3
2025-09-28T22:48:32.311562Z DEBUG beach_road::websocket: Text frame content from b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3: {"type":"signal","to_peer":"8dd3939f-bae1-48d8-82ff-6adc21787b51","signal":{"signal":{"candidate":"candidate:2839139417 1 udp 2130706431 fd7a:115c:a1e0::3601:e412 59304 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-28T22:48:32.311595Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "8dd3939f-bae1-48d8-82ff-6adc21787b51", signal: Object {"signal": Object {"candidate": String("candidate:2839139417 1 udp 2130706431 fd7a:115c:a1e0::3601:e412 59304 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-28T22:48:32.311616Z DEBUG beach_road::websocket: Received Signal from b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3 to 8dd3939f-bae1-48d8-82ff-6adc21787b51: Object {"signal": Object {"candidate": String("candidate:2839139417 1 udp 2130706431 fd7a:115c:a1e0::3601:e412 59304 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-28T22:48:32.311645Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3 to=8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:32.311720Z DEBUG beach_road::websocket: Received WebSocket frame from peer b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3
2025-09-28T22:48:32.311735Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3
2025-09-28T22:48:32.311742Z DEBUG beach_road::websocket: Text frame content from b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3: {"type":"signal","to_peer":"8dd3939f-bae1-48d8-82ff-6adc21787b51","signal":{"signal":{"candidate":"candidate:3173101093 1 udp 2130706431 192.168.68.52 52839 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-28T22:48:32.311787Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "8dd3939f-bae1-48d8-82ff-6adc21787b51", signal: Object {"signal": Object {"candidate": String("candidate:3173101093 1 udp 2130706431 192.168.68.52 52839 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-28T22:48:32.311811Z DEBUG beach_road::websocket: Received Signal from b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3 to 8dd3939f-bae1-48d8-82ff-6adc21787b51: Object {"signal": Object {"candidate": String("candidate:3173101093 1 udp 2130706431 192.168.68.52 52839 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-28T22:48:32.311848Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3 to=8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:32.311888Z DEBUG beach_road::websocket: Received WebSocket frame from peer b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3
2025-09-28T22:48:32.311896Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3
2025-09-28T22:48:32.311904Z DEBUG beach_road::websocket: Text frame content from b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3: {"type":"signal","to_peer":"8dd3939f-bae1-48d8-82ff-6adc21787b51","signal":{"signal":{"candidate":"candidate:2193730423 1 udp 2130706431 100.88.228.18 52456 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-28T22:48:32.311930Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "8dd3939f-bae1-48d8-82ff-6adc21787b51", signal: Object {"signal": Object {"candidate": String("candidate:2193730423 1 udp 2130706431 100.88.228.18 52456 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-28T22:48:32.311949Z DEBUG beach_road::websocket: Received Signal from b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3 to 8dd3939f-bae1-48d8-82ff-6adc21787b51: Object {"signal": Object {"candidate": String("candidate:2193730423 1 udp 2130706431 100.88.228.18 52456 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-28T22:48:32.312016Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3 to=8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:32.316178Z DEBUG request{method=POST uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=204
2025-09-28T22:48:32.341134Z DEBUG beach_road::websocket: Received WebSocket frame from peer b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3
2025-09-28T22:48:32.341148Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3
2025-09-28T22:48:32.341157Z DEBUG beach_road::websocket: Text frame content from b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3: {"type":"signal","to_peer":"8dd3939f-bae1-48d8-82ff-6adc21787b51","signal":{"signal":{"candidate":"candidate:2323497841 1 udp 1694498815 72.227.131.114 64855 typ srflx raddr 0.0.0.0 rport 64855","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-28T22:48:32.341195Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "8dd3939f-bae1-48d8-82ff-6adc21787b51", signal: Object {"signal": Object {"candidate": String("candidate:2323497841 1 udp 1694498815 72.227.131.114 64855 typ srflx raddr 0.0.0.0 rport 64855"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-28T22:48:32.341220Z DEBUG beach_road::websocket: Received Signal from b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3 to 8dd3939f-bae1-48d8-82ff-6adc21787b51: Object {"signal": Object {"candidate": String("candidate:2323497841 1 udp 1694498815 72.227.131.114 64855 typ srflx raddr 0.0.0.0 rport 64855"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-28T22:48:32.341255Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3 to=8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:32.517271Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:32.526474Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=9 ms status=200
2025-09-28T22:48:32.621828Z DEBUG request{method=GET uri=/ws/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038 version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:32.622046Z DEBUG request{method=GET uri=/ws/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038 version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=0 ms status=101
2025-09-28T22:48:32.622229Z DEBUG beach_road::websocket: WebSocket connected: peer=4a39cd87-ab32-423d-86aa-bb2a293d263e session=0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038
2025-09-28T22:48:32.622516Z DEBUG beach_road::websocket: Received WebSocket frame from peer 4a39cd87-ab32-423d-86aa-bb2a293d263e
2025-09-28T22:48:32.622533Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 4a39cd87-ab32-423d-86aa-bb2a293d263e
2025-09-28T22:48:32.622544Z DEBUG beach_road::websocket: Text frame content from 4a39cd87-ab32-423d-86aa-bb2a293d263e: {"type":"join","peer_id":"a44048c5-51e0-4a42-9594-59a47db706d2","passphrase":"406244","supported_transports":["webrtc"],"preferred_transport":"webrtc"}
2025-09-28T22:48:32.622593Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Join { peer_id: "a44048c5-51e0-4a42-9594-59a47db706d2", passphrase: Some("406244"), supported_transports: [WebRTC], preferred_transport: Some(WebRTC) }
2025-09-28T22:48:32.622617Z  INFO beach_road::websocket: 📥 RECEIVED Join message from peer 4a39cd87-ab32-423d-86aa-bb2a293d263e (client_peer_id: "a44048c5-51e0-4a42-9594-59a47db706d2") for session 0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038
2025-09-28T22:48:32.622669Z  INFO beach_road::websocket:   → Session has existing peers, assigning role: Client
2025-09-28T22:48:32.622689Z  INFO beach_road::websocket:   → Added peer 4a39cd87-ab32-423d-86aa-bb2a293d263e to session 0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038 with role Client
2025-09-28T22:48:32.622751Z  INFO beach_road::websocket:   → Session now has 3 peers, available transports: [WebRTC]
2025-09-28T22:48:32.622786Z  INFO beach_road::websocket: 📤 SENDING JoinSuccess to peer 4a39cd87-ab32-423d-86aa-bb2a293d263e: session=0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038, peer_id=4a39cd87-ab32-423d-86aa-bb2a293d263e, peers=3, transports=[WebRTC]
2025-09-28T22:48:32.622803Z  INFO beach_road::websocket:   → JoinSuccess sent successfully to peer 4a39cd87-ab32-423d-86aa-bb2a293d263e
2025-09-28T22:48:32.623181Z DEBUG beach_road::websocket: Received WebSocket frame from peer 4a39cd87-ab32-423d-86aa-bb2a293d263e
2025-09-28T22:48:32.623200Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 4a39cd87-ab32-423d-86aa-bb2a293d263e
2025-09-28T22:48:32.623210Z DEBUG beach_road::websocket: Text frame content from 4a39cd87-ab32-423d-86aa-bb2a293d263e: {"type":"ping"}
2025-09-28T22:48:32.623224Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Ping
2025-09-28T22:48:32.624790Z DEBUG request{method=POST uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/offer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:32.625013Z DEBUG beach_road::websocket: Received WebSocket frame from peer 4a39cd87-ab32-423d-86aa-bb2a293d263e
2025-09-28T22:48:32.625027Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 4a39cd87-ab32-423d-86aa-bb2a293d263e
2025-09-28T22:48:32.625036Z DEBUG beach_road::websocket: Text frame content from 4a39cd87-ab32-423d-86aa-bb2a293d263e: {"type":"signal","to_peer":"4a39cd87-ab32-423d-86aa-bb2a293d263e","signal":{"signal":{"candidate":"candidate:2193730423 1 udp 2130706431 100.88.228.18 58481 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-28T22:48:32.625078Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "4a39cd87-ab32-423d-86aa-bb2a293d263e", signal: Object {"signal": Object {"candidate": String("candidate:2193730423 1 udp 2130706431 100.88.228.18 58481 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-28T22:48:32.625104Z DEBUG beach_road::websocket: Received Signal from 4a39cd87-ab32-423d-86aa-bb2a293d263e to 4a39cd87-ab32-423d-86aa-bb2a293d263e: Object {"signal": Object {"candidate": String("candidate:2193730423 1 udp 2130706431 100.88.228.18 58481 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-28T22:48:32.625147Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=4a39cd87-ab32-423d-86aa-bb2a293d263e to=4a39cd87-ab32-423d-86aa-bb2a293d263e
2025-09-28T22:48:32.625179Z DEBUG beach_road::websocket: Received WebSocket frame from peer 4a39cd87-ab32-423d-86aa-bb2a293d263e
2025-09-28T22:48:32.625187Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 4a39cd87-ab32-423d-86aa-bb2a293d263e
2025-09-28T22:48:32.625224Z DEBUG beach_road::websocket: Text frame content from 4a39cd87-ab32-423d-86aa-bb2a293d263e: {"type":"signal","to_peer":"4a39cd87-ab32-423d-86aa-bb2a293d263e","signal":{"signal":{"candidate":"candidate:2839139417 1 udp 2130706431 fd7a:115c:a1e0::3601:e412 60300 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-28T22:48:32.625267Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "4a39cd87-ab32-423d-86aa-bb2a293d263e", signal: Object {"signal": Object {"candidate": String("candidate:2839139417 1 udp 2130706431 fd7a:115c:a1e0::3601:e412 60300 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-28T22:48:32.625290Z DEBUG beach_road::websocket: Received Signal from 4a39cd87-ab32-423d-86aa-bb2a293d263e to 4a39cd87-ab32-423d-86aa-bb2a293d263e: Object {"signal": Object {"candidate": String("candidate:2839139417 1 udp 2130706431 fd7a:115c:a1e0::3601:e412 60300 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-28T22:48:32.625323Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=4a39cd87-ab32-423d-86aa-bb2a293d263e to=4a39cd87-ab32-423d-86aa-bb2a293d263e
2025-09-28T22:48:32.625421Z DEBUG beach_road::websocket: Received WebSocket frame from peer 4a39cd87-ab32-423d-86aa-bb2a293d263e
2025-09-28T22:48:32.625430Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 4a39cd87-ab32-423d-86aa-bb2a293d263e
2025-09-28T22:48:32.625437Z DEBUG beach_road::websocket: Text frame content from 4a39cd87-ab32-423d-86aa-bb2a293d263e: {"type":"signal","to_peer":"4a39cd87-ab32-423d-86aa-bb2a293d263e","signal":{"signal":{"candidate":"candidate:3173101093 1 udp 2130706431 192.168.68.52 50929 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-28T22:48:32.625468Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "4a39cd87-ab32-423d-86aa-bb2a293d263e", signal: Object {"signal": Object {"candidate": String("candidate:3173101093 1 udp 2130706431 192.168.68.52 50929 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-28T22:48:32.625508Z DEBUG beach_road::websocket: Received Signal from 4a39cd87-ab32-423d-86aa-bb2a293d263e to 4a39cd87-ab32-423d-86aa-bb2a293d263e: Object {"signal": Object {"candidate": String("candidate:3173101093 1 udp 2130706431 192.168.68.52 50929 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-28T22:48:32.625548Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=4a39cd87-ab32-423d-86aa-bb2a293d263e to=4a39cd87-ab32-423d-86aa-bb2a293d263e
2025-09-28T22:48:32.628726Z DEBUG request{method=POST uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/offer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=204
2025-09-28T22:48:32.629329Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:32.629870Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=0 ms status=404
2025-09-28T22:48:32.652217Z DEBUG beach_road::websocket: Received WebSocket frame from peer 4a39cd87-ab32-423d-86aa-bb2a293d263e
2025-09-28T22:48:32.652242Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 4a39cd87-ab32-423d-86aa-bb2a293d263e
2025-09-28T22:48:32.652249Z DEBUG beach_road::websocket: Text frame content from 4a39cd87-ab32-423d-86aa-bb2a293d263e: {"type":"signal","to_peer":"4a39cd87-ab32-423d-86aa-bb2a293d263e","signal":{"signal":{"candidate":"candidate:2323497841 1 udp 1694498815 72.227.131.114 58940 typ srflx raddr 0.0.0.0 rport 58940","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-28T22:48:32.652300Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "4a39cd87-ab32-423d-86aa-bb2a293d263e", signal: Object {"signal": Object {"candidate": String("candidate:2323497841 1 udp 1694498815 72.227.131.114 58940 typ srflx raddr 0.0.0.0 rport 58940"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-28T22:48:32.652324Z DEBUG beach_road::websocket: Received Signal from 4a39cd87-ab32-423d-86aa-bb2a293d263e to 4a39cd87-ab32-423d-86aa-bb2a293d263e: Object {"signal": Object {"candidate": String("candidate:2323497841 1 udp 1694498815 72.227.131.114 58940 typ srflx raddr 0.0.0.0 rport 58940"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-28T22:48:32.652362Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=4a39cd87-ab32-423d-86aa-bb2a293d263e to=4a39cd87-ab32-423d-86aa-bb2a293d263e
2025-09-28T22:48:32.884194Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:32.888040Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-28T22:48:33.141794Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:33.144513Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-28T22:48:33.398220Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:33.399281Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=404
2025-09-28T22:48:33.654658Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:33.661300Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-28T22:48:33.916650Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:33.928539Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=12 ms status=404
2025-09-28T22:48:34.180998Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:34.187929Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-28T22:48:34.441715Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:34.445640Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-28T22:48:34.700400Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:34.704325Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-28T22:48:34.959395Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:34.965423Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-28T22:48:35.224473Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:35.234195Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=9 ms status=404
2025-09-28T22:48:35.489341Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:35.494911Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-28T22:48:35.751439Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:35.754679Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-28T22:48:36.009627Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:36.015870Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-28T22:48:36.271807Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:36.277044Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-28T22:48:36.533692Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:36.536042Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-28T22:48:36.788708Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:36.791879Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-28T22:48:37.047541Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:37.050572Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-28T22:48:37.306227Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:37.308406Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-28T22:48:37.564040Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:37.566743Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-28T22:48:37.821917Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:37.827836Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-28T22:48:38.080730Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:38.083945Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-28T22:48:38.340686Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:38.343830Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-28T22:48:38.604542Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:38.607604Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-28T22:48:38.862322Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:38.870188Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=8 ms status=404
2025-09-28T22:48:39.125949Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:39.129388Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-28T22:48:39.383429Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:39.390678Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-28T22:48:39.648746Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:39.653345Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-28T22:48:39.907725Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:39.908684Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=404
2025-09-28T22:48:40.163655Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:40.166529Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-28T22:48:40.420382Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:40.424182Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-28T22:48:40.679662Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:40.684252Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-28T22:48:40.940948Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:40.944491Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-28T22:48:41.200883Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:41.207732Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-28T22:48:41.461977Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:41.463620Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=404
2025-09-28T22:48:41.719208Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:41.732963Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=13 ms status=404
2025-09-28T22:48:41.988490Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:42.000669Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=12 ms status=404
2025-09-28T22:48:42.258211Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:42.262630Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-28T22:48:42.521217Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:42.525461Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-28T22:48:42.783069Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:42.785419Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-28T22:48:43.040724Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:43.043509Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-28T22:48:43.301129Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:43.304772Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-28T22:48:43.567888Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:43.570746Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-28T22:48:43.823771Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:43.826900Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-28T22:48:44.082723Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:44.086305Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-28T22:48:44.341857Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:44.348020Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-28T22:48:44.602587Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:44.608538Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-28T22:48:44.866559Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:44.870223Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-28T22:48:45.132783Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:45.136398Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-28T22:48:45.396841Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:45.401504Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-28T22:48:45.452638Z DEBUG request{method=OPTIONS uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/join version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:45.452728Z DEBUG request{method=OPTIONS uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/join version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=0 ms status=200
2025-09-28T22:48:45.453537Z DEBUG request{method=POST uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/join version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:45.453626Z DEBUG request{method=POST uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/join version=HTTP/1.1}: beach_road::handlers: Client attempting to join session: 0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038
2025-09-28T22:48:45.457201Z DEBUG request{method=POST uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/join version=HTTP/1.1}: beach_road::handlers: Client successfully joined session: 0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038
2025-09-28T22:48:45.457321Z DEBUG request{method=POST uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/join version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=200
2025-09-28T22:48:45.459507Z DEBUG request{method=GET uri=/ws/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038 version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:45.459613Z DEBUG request{method=GET uri=/ws/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038 version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=0 ms status=101
2025-09-28T22:48:45.459716Z DEBUG beach_road::websocket: WebSocket connected: peer=5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2 session=0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038
2025-09-28T22:48:45.460425Z DEBUG beach_road::websocket: Received WebSocket frame from peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2
2025-09-28T22:48:45.460441Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2
2025-09-28T22:48:45.460449Z DEBUG beach_road::websocket: Text frame content from 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2: {"type":"join","peer_id":"3ccfe467-faab-4133-a8e3-d1d584786442","passphrase":"406244","supported_transports":["webrtc"]}
2025-09-28T22:48:45.460487Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Join { peer_id: "3ccfe467-faab-4133-a8e3-d1d584786442", passphrase: Some("406244"), supported_transports: [WebRTC], preferred_transport: None }
2025-09-28T22:48:45.460504Z  INFO beach_road::websocket: 📥 RECEIVED Join message from peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2 (client_peer_id: "3ccfe467-faab-4133-a8e3-d1d584786442") for session 0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038
2025-09-28T22:48:45.460547Z  INFO beach_road::websocket:   → Session has existing peers, assigning role: Client
2025-09-28T22:48:45.460563Z  INFO beach_road::websocket:   → Added peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2 to session 0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038 with role Client
2025-09-28T22:48:45.460613Z  INFO beach_road::websocket:   → Session now has 4 peers, available transports: [WebRTC]
2025-09-28T22:48:45.460644Z  INFO beach_road::websocket: 📤 SENDING JoinSuccess to peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2: session=0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038, peer_id=5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2, peers=4, transports=[WebRTC]
2025-09-28T22:48:45.460657Z  INFO beach_road::websocket:   → JoinSuccess sent successfully to peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2
2025-09-28T22:48:45.461405Z DEBUG beach_road::websocket: Received WebSocket frame from peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2
2025-09-28T22:48:45.461436Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2
2025-09-28T22:48:45.461451Z DEBUG beach_road::websocket: Text frame content from 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2: {"type":"negotiate_transport","to_peer":"8dd3939f-bae1-48d8-82ff-6adc21787b51","proposed_transport":"webrtc"}
2025-09-28T22:48:45.461522Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: NegotiateTransport { to_peer: "8dd3939f-bae1-48d8-82ff-6adc21787b51", proposed_transport: WebRTC }
2025-09-28T22:48:45.465321Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/offer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:45.466567Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/offer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=200
2025-09-28T22:48:45.470316Z DEBUG request{method=OPTIONS uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:45.470401Z DEBUG request{method=OPTIONS uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=0 ms status=200
2025-09-28T22:48:45.471984Z DEBUG request{method=POST uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:45.474922Z DEBUG request{method=POST uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=204
2025-09-28T22:48:45.658496Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-28T22:48:45.664699Z DEBUG request{method=GET uri=/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=200
2025-09-28T22:48:45.876882Z DEBUG beach_road::websocket: Received WebSocket frame from peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2
2025-09-28T22:48:45.877005Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2
2025-09-28T22:48:45.877028Z DEBUG beach_road::websocket: Text frame content from 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2: {"type":"signal","to_peer":"8dd3939f-bae1-48d8-82ff-6adc21787b51","signal":{"transport":"webrtc","signal":{"signal_type":"ice_candidate","candidate":"candidate:2705597802 1 udp 2113937151 5c210355-1aca-4274-8a1c-7a9cd191743e.local 53821 typ host generation 0 ufrag urNL network-cost 999","sdp_mid":"0","sdp_mline_index":0}}}
2025-09-28T22:48:45.877269Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "8dd3939f-bae1-48d8-82ff-6adc21787b51", signal: Object {"signal": Object {"candidate": String("candidate:2705597802 1 udp 2113937151 5c210355-1aca-4274-8a1c-7a9cd191743e.local 53821 typ host generation 0 ufrag urNL network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-28T22:48:45.878370Z DEBUG beach_road::websocket: Received Signal from 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2 to 8dd3939f-bae1-48d8-82ff-6adc21787b51: Object {"signal": Object {"candidate": String("candidate:2705597802 1 udp 2113937151 5c210355-1aca-4274-8a1c-7a9cd191743e.local 53821 typ host generation 0 ufrag urNL network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-28T22:48:45.878573Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2 to=8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:45.879805Z DEBUG beach_road::websocket: Received WebSocket frame from peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2
2025-09-28T22:48:45.879836Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2
2025-09-28T22:48:45.881468Z DEBUG beach_road::websocket: Text frame content from 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2: {"type":"signal","to_peer":"8dd3939f-bae1-48d8-82ff-6adc21787b51","signal":{"transport":"webrtc","signal":{"signal_type":"ice_candidate","candidate":"candidate:2028413328 1 udp 1677729535 72.227.131.114 53821 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag urNL network-cost 999","sdp_mid":"0","sdp_mline_index":0}}}
2025-09-28T22:48:45.881833Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "8dd3939f-bae1-48d8-82ff-6adc21787b51", signal: Object {"signal": Object {"candidate": String("candidate:2028413328 1 udp 1677729535 72.227.131.114 53821 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag urNL network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-28T22:48:45.882773Z DEBUG beach_road::websocket: Received Signal from 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2 to 8dd3939f-bae1-48d8-82ff-6adc21787b51: Object {"signal": Object {"candidate": String("candidate:2028413328 1 udp 1677729535 72.227.131.114 53821 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag urNL network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-28T22:48:45.882895Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2 to=8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:47.087455Z DEBUG beach_road::websocket: Received WebSocket frame from peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2
2025-09-28T22:48:47.087602Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2
2025-09-28T22:48:47.087625Z DEBUG beach_road::websocket: Text frame content from 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2: {"type":"signal","to_peer":"8dd3939f-bae1-48d8-82ff-6adc21787b51","signal":{"transport":"webrtc","signal":{"signal_type":"ice_candidate","candidate":"candidate:2705597802 1 udp 2113937151 5c210355-1aca-4274-8a1c-7a9cd191743e.local 53821 typ host generation 0 ufrag urNL network-cost 999","sdp_mid":"0","sdp_mline_index":0}}}
2025-09-28T22:48:47.087857Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "8dd3939f-bae1-48d8-82ff-6adc21787b51", signal: Object {"signal": Object {"candidate": String("candidate:2705597802 1 udp 2113937151 5c210355-1aca-4274-8a1c-7a9cd191743e.local 53821 typ host generation 0 ufrag urNL network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-28T22:48:47.087958Z DEBUG beach_road::websocket: Received Signal from 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2 to 8dd3939f-bae1-48d8-82ff-6adc21787b51: Object {"signal": Object {"candidate": String("candidate:2705597802 1 udp 2113937151 5c210355-1aca-4274-8a1c-7a9cd191743e.local 53821 typ host generation 0 ufrag urNL network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-28T22:48:47.088219Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2 to=8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:47.088981Z DEBUG beach_road::websocket: Received WebSocket frame from peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2
2025-09-28T22:48:47.089009Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2
2025-09-28T22:48:47.089027Z DEBUG beach_road::websocket: Text frame content from 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2: {"type":"signal","to_peer":"8dd3939f-bae1-48d8-82ff-6adc21787b51","signal":{"transport":"webrtc","signal":{"signal_type":"ice_candidate","candidate":"candidate:2028413328 1 udp 1677729535 72.227.131.114 53821 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag urNL network-cost 999","sdp_mid":"0","sdp_mline_index":0}}}
2025-09-28T22:48:47.089114Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "8dd3939f-bae1-48d8-82ff-6adc21787b51", signal: Object {"signal": Object {"candidate": String("candidate:2028413328 1 udp 1677729535 72.227.131.114 53821 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag urNL network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-28T22:48:47.090147Z DEBUG beach_road::websocket: Received Signal from 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2 to 8dd3939f-bae1-48d8-82ff-6adc21787b51: Object {"signal": Object {"candidate": String("candidate:2028413328 1 udp 1677729535 72.227.131.114 53821 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag urNL network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-28T22:48:47.090299Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2 to=8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:48.299317Z DEBUG beach_road::websocket: Received WebSocket frame from peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2
2025-09-28T22:48:48.299439Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2
2025-09-28T22:48:48.299462Z DEBUG beach_road::websocket: Text frame content from 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2: {"type":"signal","to_peer":"8dd3939f-bae1-48d8-82ff-6adc21787b51","signal":{"transport":"webrtc","signal":{"signal_type":"ice_candidate","candidate":"candidate:2705597802 1 udp 2113937151 5c210355-1aca-4274-8a1c-7a9cd191743e.local 53821 typ host generation 0 ufrag urNL network-cost 999","sdp_mid":"0","sdp_mline_index":0}}}
2025-09-28T22:48:48.299707Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "8dd3939f-bae1-48d8-82ff-6adc21787b51", signal: Object {"signal": Object {"candidate": String("candidate:2705597802 1 udp 2113937151 5c210355-1aca-4274-8a1c-7a9cd191743e.local 53821 typ host generation 0 ufrag urNL network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-28T22:48:48.299808Z DEBUG beach_road::websocket: Received Signal from 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2 to 8dd3939f-bae1-48d8-82ff-6adc21787b51: Object {"signal": Object {"candidate": String("candidate:2705597802 1 udp 2113937151 5c210355-1aca-4274-8a1c-7a9cd191743e.local 53821 typ host generation 0 ufrag urNL network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-28T22:48:48.300018Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2 to=8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:48.300136Z DEBUG beach_road::websocket: Received WebSocket frame from peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2
2025-09-28T22:48:48.300155Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2
2025-09-28T22:48:48.300172Z DEBUG beach_road::websocket: Text frame content from 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2: {"type":"signal","to_peer":"8dd3939f-bae1-48d8-82ff-6adc21787b51","signal":{"transport":"webrtc","signal":{"signal_type":"ice_candidate","candidate":"candidate:2028413328 1 udp 1677729535 72.227.131.114 53821 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag urNL network-cost 999","sdp_mid":"0","sdp_mline_index":0}}}
2025-09-28T22:48:48.300309Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "8dd3939f-bae1-48d8-82ff-6adc21787b51", signal: Object {"signal": Object {"candidate": String("candidate:2028413328 1 udp 1677729535 72.227.131.114 53821 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag urNL network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-28T22:48:48.300373Z DEBUG beach_road::websocket: Received Signal from 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2 to 8dd3939f-bae1-48d8-82ff-6adc21787b51: Object {"signal": Object {"candidate": String("candidate:2028413328 1 udp 1677729535 72.227.131.114 53821 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag urNL network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-28T22:48:48.300562Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2 to=8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:49.502331Z DEBUG beach_road::websocket: Received WebSocket frame from peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2
2025-09-28T22:48:49.502490Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2
2025-09-28T22:48:49.502534Z DEBUG beach_road::websocket: Text frame content from 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2: {"type":"signal","to_peer":"8dd3939f-bae1-48d8-82ff-6adc21787b51","signal":{"transport":"webrtc","signal":{"signal_type":"ice_candidate","candidate":"candidate:2705597802 1 udp 2113937151 5c210355-1aca-4274-8a1c-7a9cd191743e.local 53821 typ host generation 0 ufrag urNL network-cost 999","sdp_mid":"0","sdp_mline_index":0}}}
2025-09-28T22:48:49.502917Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "8dd3939f-bae1-48d8-82ff-6adc21787b51", signal: Object {"signal": Object {"candidate": String("candidate:2705597802 1 udp 2113937151 5c210355-1aca-4274-8a1c-7a9cd191743e.local 53821 typ host generation 0 ufrag urNL network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-28T22:48:49.503087Z DEBUG beach_road::websocket: Received Signal from 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2 to 8dd3939f-bae1-48d8-82ff-6adc21787b51: Object {"signal": Object {"candidate": String("candidate:2705597802 1 udp 2113937151 5c210355-1aca-4274-8a1c-7a9cd191743e.local 53821 typ host generation 0 ufrag urNL network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-28T22:48:49.503752Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2 to=8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:49.503985Z DEBUG beach_road::websocket: Received WebSocket frame from peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2
2025-09-28T22:48:49.504026Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2
2025-09-28T22:48:49.504068Z DEBUG beach_road::websocket: Text frame content from 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2: {"type":"signal","to_peer":"8dd3939f-bae1-48d8-82ff-6adc21787b51","signal":{"transport":"webrtc","signal":{"signal_type":"ice_candidate","candidate":"candidate:2028413328 1 udp 1677729535 72.227.131.114 53821 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag urNL network-cost 999","sdp_mid":"0","sdp_mline_index":0}}}
2025-09-28T22:48:49.504231Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "8dd3939f-bae1-48d8-82ff-6adc21787b51", signal: Object {"signal": Object {"candidate": String("candidate:2028413328 1 udp 1677729535 72.227.131.114 53821 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag urNL network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-28T22:48:49.504367Z DEBUG beach_road::websocket: Received Signal from 5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2 to 8dd3939f-bae1-48d8-82ff-6adc21787b51: Object {"signal": Object {"candidate": String("candidate:2028413328 1 udp 1677729535 72.227.131.114 53821 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag urNL network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-28T22:48:49.504527Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2 to=8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:54.982774Z DEBUG beach_road::websocket: Received WebSocket frame from peer 8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:54.982918Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 8dd3939f-bae1-48d8-82ff-6adc21787b51
2025-09-28T22:48:54.982945Z DEBUG beach_road::websocket: Text frame content from 8dd3939f-bae1-48d8-82ff-6adc21787b51: {"type":"ping"}
2025-09-28T22:48:54.983118Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Ping
2025-09-28T22:49:02.311209Z DEBUG beach_road::websocket: Received WebSocket frame from peer b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3
2025-09-28T22:49:02.311390Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3
2025-09-28T22:49:02.311436Z DEBUG beach_road::websocket: Text frame content from b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3: {"type":"ping"}
2025-09-28T22:49:02.311551Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Ping
2025-09-28T22:49:02.624601Z DEBUG beach_road::websocket: Received WebSocket frame from peer 4a39cd87-ab32-423d-86aa-bb2a293d263e
2025-09-28T22:49:02.624694Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 4a39cd87-ab32-423d-86aa-bb2a293d263e
2025-09-28T22:49:02.624906Z DEBUG beach_road::websocket: Text frame content from 4a39cd87-ab32-423d-86aa-bb2a293d263e: {"type":"ping"}
2025-09-28T22:49:02.624979Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Ping
```

server: ```(base) arellidow@Arels-MacBook-Pro beach % cd ~/development/beach/apps/beach

cargo run -- \
  --session-server http://127.0.0.1:8080 \
  --log-file ~/beach-debug/host.log
   Compiling beach v0.1.0 (/Users/arellidow/development/beach/apps/beach)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 8.88s
     Running `/Users/arellidow/development/beach/target/debug/beach --session-server 'http://127.0.0.1:8080' --log-file /Users/arellidow/beach-debug/host.log`

🏖️  beach session ready!

                         session id : 0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038
                                                                            share url  : http://127.0.0.1:8080/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038
                                                      passcode   : 406244

                                                                           share command:
                                                                                             beach --session-server http://127.0.0.1:8080/ join 0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038 --passcode 406244

                                                                                                transports : WebRTC, WebSocket
                        active     : WebRTC

                                           🌊 Launching host process... type 'exit' to end the session.

                                                                                                       Restored session: Sun Sep 28 18:48:13 EDT 2025
(base) arellidow@Arels-MacBook-Pro ~ % echo hi
hi
(base) arellidow@Arels-MacBook-Pro ~ % ```

console log: ```chunk-V2JXGMUL.js?v=5c52638e:21549 Download the React DevTools for a better development experience: https://reactjs.org/link/react-devtools
favicon.ico:1  GET http://localhost:5173/favicon.ico 404 (Not Found)
BeachTerminal.tsx:89 [beach-surfer] join_success payload: {"type":"join_success","session_id":"0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038","peer_id":"5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2","peers":[{"id":"4a39cd87-ab32-423d-86aa-bb2a293d263e","role":"client","joined_at":1759099725,"supported_transports":["webrtc"],"preferred_transport":"webrtc"},{"id":"8dd3939f-bae1-48d8-82ff-6adc21787b51","role":"server","joined_at":1759099725,"supported_transports":["webrtc"],"preferred_transport":"webrtc"},{"id":"5b1e4ad7-5e14-4b89-be6b-8c5a6a5762e2","role":"client","joined_at":1759099725,"supported_transports":["webrtc"],"preferred_transport":null},{"id":"b5a95d8d-5dbf-44fa-b762-8f1b0d6fd7e3","role":"client","joined_at":1759099725,"supported_transports":["webrtc"],"preferred_transport":"webrtc"}],"available_transports":["webrtc"]}
BeachTerminal.tsx:89 [beach-surfer] remote peer resolved: 8dd3939f-bae1-48d8-82ff-6adc21787b51
BeachTerminal.tsx:89 [beach-surfer] polling for SDP offer
BeachTerminal.tsx:89 [beach-surfer] polled SDP at http://127.0.0.1:8080/sessions/0dd900a5-ce1b-4fd6-b4af-e9f87aa6a038/webrtc/offer
BeachTerminal.tsx:89 [beach-surfer] SDP offer received
BeachTerminal.tsx:89 [beach-surfer] waiting for data channel announcement
BeachTerminal.tsx:89 [beach-surfer] signaling state: have-remote-offer
BeachTerminal.tsx:89 [beach-surfer] signaling state: stable
BeachTerminal.tsx:89 [beach-surfer] ice gathering state: gathering
BeachTerminal.tsx:89 [beach-surfer] local candidate queued: {"candidate":"candidate:2705597802 1 udp 2113937151 5c210355-1aca-4274-8a1c-7a9cd191743e.local 53821 typ host generation 0 ufrag urNL network-cost 999","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":"urNL"}
BeachTerminal.tsx:89 [beach-surfer] SDP answer posted
BeachTerminal.tsx:89 [beach-surfer] local candidate queued: {"candidate":"candidate:2028413328 1 udp 1677729535 72.227.131.114 53821 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag urNL network-cost 999","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":"urNL"}
BeachTerminal.tsx:89 [beach-surfer] sending local candidate: {"candidate":"candidate:2705597802 1 udp 2113937151 5c210355-1aca-4274-8a1c-7a9cd191743e.local 53821 typ host generation 0 ufrag urNL network-cost 999","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":"urNL"}
BeachTerminal.tsx:89 [beach-surfer] sending local candidate: {"candidate":"candidate:2028413328 1 udp 1677729535 72.227.131.114 53821 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag urNL network-cost 999","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":"urNL"}
BeachTerminal.tsx:89 [beach-surfer] sending local candidate: {"candidate":"candidate:2705597802 1 udp 2113937151 5c210355-1aca-4274-8a1c-7a9cd191743e.local 53821 typ host generation 0 ufrag urNL network-cost 999","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":"urNL"}
BeachTerminal.tsx:89 [beach-surfer] sending local candidate: {"candidate":"candidate:2028413328 1 udp 1677729535 72.227.131.114 53821 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag urNL network-cost 999","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":"urNL"}
BeachTerminal.tsx:89 [beach-surfer] sending local candidate: {"candidate":"candidate:2705597802 1 udp 2113937151 5c210355-1aca-4274-8a1c-7a9cd191743e.local 53821 typ host generation 0 ufrag urNL network-cost 999","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":"urNL"}
BeachTerminal.tsx:89 [beach-surfer] sending local candidate: {"candidate":"candidate:2028413328 1 udp 1677729535 72.227.131.114 53821 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag urNL network-cost 999","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":"urNL"}
BeachTerminal.tsx:89 [beach-surfer] sending local candidate: {"candidate":"candidate:2705597802 1 udp 2113937151 5c210355-1aca-4274-8a1c-7a9cd191743e.local 53821 typ host generation 0 ufrag urNL network-cost 999","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":"urNL"}
BeachTerminal.tsx:89 [beach-surfer] sending local candidate: {"candidate":"candidate:2028413328 1 udp 1677729535 72.227.131.114 53821 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag urNL network-cost 999","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":"urNL"}
BeachTerminal.tsx:89 [beach-surfer] ice gathering state: complete```

important: i connected the rust client first, then connected the beach-surfer client. it's only if i connect beach web AFTER connecting the rust client that it times out

Diagnosis (2025-09-29):
- The host-side signaling client (`apps/beach/src/transport/webrtc/signaling.rs`) freezes the first client peer ID it observes and keeps routing WebRTC signals to that peer. When the Rust CLI joins before beach-surfer, the offerer never retargets, so the browser's answer/ICE/readiness frames are dropped. The offerer times out waiting for `__ready__`, continues to speak into a closed channel, and the new client never finishes handshaking.

Proposed fix:
- Teach the signaling helper to adopt the peer that is actually negotiating WebRTC: pick up the requester from `transport_proposal`, switch to whichever peer sends WebRTC `signal` frames, and clear the assignment when that participant drops. Once the remote peer ID follows the current negotiator, the browser's signals reach the offerer and the readiness exchange should succeed.

Update (2025-09-29, evening):
- The above change still left the browser stuck because the offerer reused a single `RTCPeerConnection` forever. Once the first client answered, every later peer kept polling `/webrtc/answer` (`23:07:27-45` in the road logs) but never saw a `data channel announced` event—`waitForDataChannel` timed out because the original connection was still bound to the Rust CLI.
- We now track a `remote_generation` on the signaling client and reworked the offerer so it waits for the active peer, restarts the SDP exchange whenever that generation changes, and aborts if a new peer arrives mid-handshake. The new loop posts a fresh offer, pulls the matching answer, and only proceeds once the data channel for the current generation is open.

---

Latest most recent update:

fixes didn't work.

ok fixed the rust client connection issue, but the beach-surfer client still times out if it doesn't connect first

Console: ```chunk-V2JXGMUL.js?v=5c52638e:21549 Download the React DevTools for a better development experience: https://reactjs.org/link/react-devtools
favicon.ico:1  GET http://localhost:5173/favicon.ico 404 (Not Found)
BeachTerminal.tsx:89 [beach-surfer] join_success payload: {"type":"join_success","session_id":"37947890-7cc8-41e0-a1aa-007377847a99","peer_id":"ddc11e3a-0600-4319-ad59-edc8f9d5d914","peers":[{"id":"3ff99b94-7d73-4928-b6ac-14d706034172","role":"server","joined_at":1759105754,"supported_transports":["webrtc"],"preferred_transport":"webrtc"},{"id":"353847d9-67b8-4bb0-a33a-ecf99b651999","role":"client","joined_at":1759105754,"supported_transports":["webrtc"],"preferred_transport":"webrtc"},{"id":"93d239af-739e-41bd-a047-c2588aacf488","role":"client","joined_at":1759105754,"supported_transports":["webrtc"],"preferred_transport":"webrtc"},{"id":"ddc11e3a-0600-4319-ad59-edc8f9d5d914","role":"client","joined_at":1759105754,"supported_transports":["webrtc"],"preferred_transport":null}],"available_transports":["webrtc"]}
BeachTerminal.tsx:89 [beach-surfer] remote peer resolved: 3ff99b94-7d73-4928-b6ac-14d706034172
BeachTerminal.tsx:89 [beach-surfer] polling for SDP offer
BeachTerminal.tsx:89 [beach-surfer] polled SDP at http://127.0.0.1:8080/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/offer
BeachTerminal.tsx:89 [beach-surfer] SDP offer received
BeachTerminal.tsx:89 [beach-surfer] waiting for data channel announcement
BeachTerminal.tsx:89 [beach-surfer] signaling state: have-remote-offer
BeachTerminal.tsx:89 [beach-surfer] signaling state: stable
BeachTerminal.tsx:89 [beach-surfer] ice gathering state: gathering
BeachTerminal.tsx:89 [beach-surfer] local candidate queued: {"candidate":"candidate:5606886 1 udp 2113937151 f48d1ff3-9ed3-45c2-a73f-a9f7ac97b2e4.local 62250 typ host generation 0 ufrag Q7sc network-cost 999","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":"Q7sc"}
BeachTerminal.tsx:89 [beach-surfer] SDP answer posted
BeachTerminal.tsx:89 [beach-surfer] local candidate queued: {"candidate":"candidate:3656812828 1 udp 1677729535 72.227.131.114 62250 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag Q7sc network-cost 999","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":"Q7sc"}
BeachTerminal.tsx:89 [beach-surfer] signal message: {"signal":{"candidate":"candidate:2193730423 1 udp 2130706431 100.88.228.18 56916 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}
BeachTerminal.tsx:89 [beach-surfer] signal message: {"signal":{"candidate":"candidate:2839139417 1 udp 2130706431 fd7a:115c:a1e0::3601:e412 61891 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}
BeachTerminal.tsx:89 [beach-surfer] signal message: {"signal":{"candidate":"candidate:3173101093 1 udp 2130706431 192.168.68.52 59052 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}
BeachTerminal.tsx:89 [beach-surfer] signal message: {"signal":{"candidate":"candidate:2323497841 1 udp 1694498815 72.227.131.114 61132 typ srflx raddr 0.0.0.0 rport 61132","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}
BeachTerminal.tsx:89 [beach-surfer] sending local candidate: {"candidate":"candidate:5606886 1 udp 2113937151 f48d1ff3-9ed3-45c2-a73f-a9f7ac97b2e4.local 62250 typ host generation 0 ufrag Q7sc network-cost 999","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":"Q7sc"}
BeachTerminal.tsx:89 [beach-surfer] sending local candidate: {"candidate":"candidate:3656812828 1 udp 1677729535 72.227.131.114 62250 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag Q7sc network-cost 999","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":"Q7sc"}
BeachTerminal.tsx:89 [beach-surfer] sending local candidate: {"candidate":"candidate:5606886 1 udp 2113937151 f48d1ff3-9ed3-45c2-a73f-a9f7ac97b2e4.local 62250 typ host generation 0 ufrag Q7sc network-cost 999","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":"Q7sc"}
BeachTerminal.tsx:89 [beach-surfer] sending local candidate: {"candidate":"candidate:3656812828 1 udp 1677729535 72.227.131.114 62250 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag Q7sc network-cost 999","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":"Q7sc"}
BeachTerminal.tsx:89 [beach-surfer] sending local candidate: {"candidate":"candidate:5606886 1 udp 2113937151 f48d1ff3-9ed3-45c2-a73f-a9f7ac97b2e4.local 62250 typ host generation 0 ufrag Q7sc network-cost 999","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":"Q7sc"}
BeachTerminal.tsx:89 [beach-surfer] sending local candidate: {"candidate":"candidate:3656812828 1 udp 1677729535 72.227.131.114 62250 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag Q7sc network-cost 999","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":"Q7sc"}
BeachTerminal.tsx:89 [beach-surfer] sending local candidate: {"candidate":"candidate:5606886 1 udp 2113937151 f48d1ff3-9ed3-45c2-a73f-a9f7ac97b2e4.local 62250 typ host generation 0 ufrag Q7sc network-cost 999","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":"Q7sc"}
BeachTerminal.tsx:89 [beach-surfer] sending local candidate: {"candidate":"candidate:3656812828 1 udp 1677729535 72.227.131.114 62250 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag Q7sc network-cost 999","sdpMid":"0","sdpMLineIndex":0,"usernameFragment":"Q7sc"}```

server: ```(base) arellidow@Arels-MacBook-Pro beach % cd ~/development/beach/apps/beach

cargo run -- \
  --session-server http://127.0.0.1:8080 \
  --log-file ~/beach-debug/host.log
   Compiling beach v0.1.0 (/Users/arellidow/development/beach/apps/beach)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 7.45s
     Running `/Users/arellidow/development/beach/target/debug/beach --session-server 'http://127.0.0.1:8080' --log-file /Users/arellidow/beach-debug/host.log`

🏖️  beach session ready!

                         session id : 37947890-7cc8-41e0-a1aa-007377847a99
                                                                            share url  : http://127.0.0.1:8080/sessions/37947890-7cc8-41e0-a1aa-007377847a99
                                                      passcode   : 204220

                                                                           share command:
                                                                                             beach --session-server http://127.0.0.1:8080/ join 37947890-7cc8-41e0-a1aa-007377847a99 --passcode 204220

                                                                                                transports : WebRTC, WebSocket
                        active     : WebRTC

                                           🌊 Launching host process... type 'exit' to end the session.

                                                                                                       Restored session: Sun Sep 28 20:13:12 EDT 2025
(base) arellidow@Arels-MacBook-Pro ~ % echo hi
hi
(base) arellidow@Arels-MacBook-Pro ~ % 
```

beach-road: ```(base) arellidow@Arels-MacBook-Pro beach-road % cargo run
warning: unused import: `info`
 --> apps/beach-road/src/cli.rs:7:29
  |
7 | use tracing::{debug, error, info};
  |                             ^^^^
  |
  = note: `#[warn(unused_imports)]` on by default

warning: unused import: `info`
  --> apps/beach-road/src/handlers.rs:11:29
   |
11 | use tracing::{debug, error, info};
   |                             ^^^^

warning: unused import: `trace`
  --> apps/beach-road/src/websocket.rs:13:35
   |
13 | use tracing::{debug, error, info, trace, warn};
   |                                   ^^^^^

warning: unused variable: `from_peer`
   --> apps/beach-road/src/cli.rs:203:49
    |
203 |                         ServerMessage::Signal { from_peer, signal } => {
    |                                                 ^^^^^^^^^ help: try ignoring the field: `from_peer: _`
    |
    = note: `#[warn(unused_variables)]` on by default

warning: variant `Ipc` is never constructed
  --> apps/beach-road/src/handlers.rs:25:5
   |
22 | pub enum AdvertisedTransportKind {
   |          ----------------------- variant in this enum
...
25 |     Ipc,
   |     ^^^
   |
   = note: `AdvertisedTransportKind` has derived impls for the traits `Clone` and `Debug`, but these are intentionally ignored during dead code analysis
   = note: `#[warn(dead_code)]` on by default

warning: function `generate_session_id` is never used
 --> apps/beach-road/src/session.rs:5:8
  |
5 | pub fn generate_session_id() -> String {
  |        ^^^^^^^^^^^^^^^^^^^

warning: methods `delete_session` and `clear_webrtc_offer` are never used
   --> apps/beach-road/src/storage.rs:78:18
    |
39  | impl Storage {
    | ------------ methods in this implementation
...
78  |     pub async fn delete_session(&mut self, session_id: &str) -> Result<()> {
    |                  ^^^^^^^^^^^^^^
...
106 |     pub async fn clear_webrtc_offer(&mut self, session_id: &str) -> Result<()> {
    |                  ^^^^^^^^^^^^^^^^^^

warning: field `session_id` is never read
  --> apps/beach-road/src/websocket.rs:25:5
   |
23 | struct PeerConnection {
   |        -------------- field in this struct
24 |     peer_id: String,
25 |     session_id: String,
   |     ^^^^^^^^^^
   |
   = note: `PeerConnection` has a derived impl for the trait `Clone`, but this is intentionally ignored during dead code analysis

warning: field `storage` is never read
  --> apps/beach-road/src/websocket.rs:39:5
   |
35 | pub struct SignalingState {
   |            -------------- field in this struct
...
39 |     storage: SharedStorage,
   |     ^^^^^^^
   |
   = note: `SignalingState` has a derived impl for the trait `Clone`, but this is intentionally ignored during dead code analysis

warning: `beach-road` (bin "beach-road") generated 9 warnings (run `cargo fix --bin "beach-road"` to apply 3 suggestions)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.13s
     Running `/Users/arellidow/development/beach/target/debug/beach-road`
2025-09-29T00:28:32.920316Z  INFO beach_road: Starting Beach Road session server on port 8080
2025-09-29T00:28:32.920460Z  INFO beach_road: Redis URL: redis://localhost:6379
2025-09-29T00:28:32.920496Z  INFO beach_road: Session TTL: 3600 seconds
2025-09-29T00:28:32.930195Z  INFO beach_road: Beach Road listening on 0.0.0.0:8080
🏖️  Beach Road listening on 0.0.0.0:8080
2025-09-29T00:28:45.254527Z DEBUG request{method=POST uri=/sessions version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:28:45.254750Z DEBUG request{method=POST uri=/sessions version=HTTP/1.1}: beach_road::handlers: Registering session: 37947890-7cc8-41e0-a1aa-007377847a99
2025-09-29T00:28:45.257909Z DEBUG request{method=POST uri=/sessions version=HTTP/1.1}: beach_road::handlers: Session 37947890-7cc8-41e0-a1aa-007377847a99 registered successfully
2025-09-29T00:28:45.257979Z DEBUG request{method=POST uri=/sessions version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=200
2025-09-29T00:28:45.265168Z DEBUG request{method=GET uri=/ws/37947890-7cc8-41e0-a1aa-007377847a99 version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:28:45.265285Z DEBUG request{method=GET uri=/ws/37947890-7cc8-41e0-a1aa-007377847a99 version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=0 ms status=101
2025-09-29T00:28:45.265404Z DEBUG beach_road::websocket: WebSocket connected: peer=3ff99b94-7d73-4928-b6ac-14d706034172 session=37947890-7cc8-41e0-a1aa-007377847a99
2025-09-29T00:28:45.265964Z DEBUG beach_road::websocket: Received WebSocket frame from peer 3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:28:45.265986Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:28:45.265994Z DEBUG beach_road::websocket: Text frame content from 3ff99b94-7d73-4928-b6ac-14d706034172: {"type":"join","peer_id":"1ff4c4c5-4101-4fec-85a0-3f80aa1c575f","passphrase":"204220","supported_transports":["webrtc"],"preferred_transport":"webrtc"}
2025-09-29T00:28:45.266198Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Join { peer_id: "1ff4c4c5-4101-4fec-85a0-3f80aa1c575f", passphrase: Some("204220"), supported_transports: [WebRTC], preferred_transport: Some(WebRTC) }
2025-09-29T00:28:45.266212Z  INFO beach_road::websocket: 📥 RECEIVED Join message from peer 3ff99b94-7d73-4928-b6ac-14d706034172 (client_peer_id: "1ff4c4c5-4101-4fec-85a0-3f80aa1c575f") for session 37947890-7cc8-41e0-a1aa-007377847a99
2025-09-29T00:28:45.266225Z  INFO beach_road::websocket:   → First peer in session, assigning role: Server
2025-09-29T00:28:45.266249Z  INFO beach_road::websocket:   → Added peer 3ff99b94-7d73-4928-b6ac-14d706034172 to session 37947890-7cc8-41e0-a1aa-007377847a99 with role Server
2025-09-29T00:28:45.266342Z  INFO beach_road::websocket:   → Session now has 1 peers, available transports: [WebRTC]
2025-09-29T00:28:45.266374Z  INFO beach_road::websocket: 📤 SENDING JoinSuccess to peer 3ff99b94-7d73-4928-b6ac-14d706034172: session=37947890-7cc8-41e0-a1aa-007377847a99, peer_id=3ff99b94-7d73-4928-b6ac-14d706034172, peers=1, transports=[WebRTC]
2025-09-29T00:28:45.266387Z  INFO beach_road::websocket:   → JoinSuccess sent successfully to peer 3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:28:45.267030Z DEBUG beach_road::websocket: Received WebSocket frame from peer 3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:28:45.267042Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:28:45.267049Z DEBUG beach_road::websocket: Text frame content from 3ff99b94-7d73-4928-b6ac-14d706034172: {"type":"ping"}
2025-09-29T00:28:45.267060Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Ping
2025-09-29T00:28:45.273544Z DEBUG request{method=POST uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/offer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:28:45.277083Z DEBUG request{method=POST uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/offer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=204
2025-09-29T00:28:56.933635Z DEBUG request{method=POST uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/join version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:28:56.933761Z DEBUG request{method=POST uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/join version=HTTP/1.1}: beach_road::handlers: Client attempting to join session: 37947890-7cc8-41e0-a1aa-007377847a99
2025-09-29T00:28:56.936478Z DEBUG request{method=POST uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/join version=HTTP/1.1}: beach_road::handlers: Client successfully joined session: 37947890-7cc8-41e0-a1aa-007377847a99
2025-09-29T00:28:56.936595Z DEBUG request{method=POST uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/join version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=200
2025-09-29T00:28:56.941228Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/offer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:28:56.942389Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/offer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=200
2025-09-29T00:28:56.949895Z DEBUG request{method=GET uri=/ws/37947890-7cc8-41e0-a1aa-007377847a99 version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:28:56.950019Z DEBUG request{method=GET uri=/ws/37947890-7cc8-41e0-a1aa-007377847a99 version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=0 ms status=101
2025-09-29T00:28:56.950140Z DEBUG beach_road::websocket: WebSocket connected: peer=353847d9-67b8-4bb0-a33a-ecf99b651999 session=37947890-7cc8-41e0-a1aa-007377847a99
2025-09-29T00:28:56.951261Z DEBUG beach_road::websocket: Received WebSocket frame from peer 353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:28:56.951276Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:28:56.951286Z DEBUG beach_road::websocket: Text frame content from 353847d9-67b8-4bb0-a33a-ecf99b651999: {"type":"join","peer_id":"3649263a-86a3-4f33-ba40-707688e2458b","passphrase":"204220","supported_transports":["webrtc"],"preferred_transport":"webrtc"}
2025-09-29T00:28:56.951327Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Join { peer_id: "3649263a-86a3-4f33-ba40-707688e2458b", passphrase: Some("204220"), supported_transports: [WebRTC], preferred_transport: Some(WebRTC) }
2025-09-29T00:28:56.951350Z  INFO beach_road::websocket: 📥 RECEIVED Join message from peer 353847d9-67b8-4bb0-a33a-ecf99b651999 (client_peer_id: "3649263a-86a3-4f33-ba40-707688e2458b") for session 37947890-7cc8-41e0-a1aa-007377847a99
2025-09-29T00:28:56.951403Z  INFO beach_road::websocket:   → Session has existing peers, assigning role: Client
2025-09-29T00:28:56.951425Z  INFO beach_road::websocket:   → Added peer 353847d9-67b8-4bb0-a33a-ecf99b651999 to session 37947890-7cc8-41e0-a1aa-007377847a99 with role Client
2025-09-29T00:28:56.951486Z  INFO beach_road::websocket:   → Session now has 2 peers, available transports: [WebRTC]
2025-09-29T00:28:56.951520Z  INFO beach_road::websocket: 📤 SENDING JoinSuccess to peer 353847d9-67b8-4bb0-a33a-ecf99b651999: session=37947890-7cc8-41e0-a1aa-007377847a99, peer_id=353847d9-67b8-4bb0-a33a-ecf99b651999, peers=2, transports=[WebRTC]
2025-09-29T00:28:56.951535Z  INFO beach_road::websocket:   → JoinSuccess sent successfully to peer 353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:28:56.952136Z DEBUG beach_road::websocket: Received WebSocket frame from peer 3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:28:56.952159Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:28:56.952170Z DEBUG beach_road::websocket: Text frame content from 3ff99b94-7d73-4928-b6ac-14d706034172: {"type":"signal","to_peer":"353847d9-67b8-4bb0-a33a-ecf99b651999","signal":{"signal":{"candidate":"candidate:3173101093 1 udp 2130706431 192.168.68.52 52855 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-29T00:28:56.952240Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "353847d9-67b8-4bb0-a33a-ecf99b651999", signal: Object {"signal": Object {"candidate": String("candidate:3173101093 1 udp 2130706431 192.168.68.52 52855 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:28:56.952293Z DEBUG beach_road::websocket: Received Signal from 3ff99b94-7d73-4928-b6ac-14d706034172 to 353847d9-67b8-4bb0-a33a-ecf99b651999: Object {"signal": Object {"candidate": String("candidate:3173101093 1 udp 2130706431 192.168.68.52 52855 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:28:56.952305Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:28:56.952358Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=3ff99b94-7d73-4928-b6ac-14d706034172 to=353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:28:56.952394Z DEBUG beach_road::websocket: Received WebSocket frame from peer 3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:28:56.952402Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:28:56.952410Z DEBUG beach_road::websocket: Text frame content from 3ff99b94-7d73-4928-b6ac-14d706034172: {"type":"signal","to_peer":"353847d9-67b8-4bb0-a33a-ecf99b651999","signal":{"signal":{"candidate":"candidate:2839139417 1 udp 2130706431 fd7a:115c:a1e0::3601:e412 55705 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-29T00:28:56.952461Z DEBUG beach_road::websocket: Received WebSocket frame from peer 353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:28:56.952472Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:28:56.952480Z DEBUG beach_road::websocket: Text frame content from 353847d9-67b8-4bb0-a33a-ecf99b651999: {"type":"ping"}
2025-09-29T00:28:56.952492Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Ping
2025-09-29T00:28:56.952477Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "353847d9-67b8-4bb0-a33a-ecf99b651999", signal: Object {"signal": Object {"candidate": String("candidate:2839139417 1 udp 2130706431 fd7a:115c:a1e0::3601:e412 55705 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:28:56.952571Z DEBUG beach_road::websocket: Received Signal from 3ff99b94-7d73-4928-b6ac-14d706034172 to 353847d9-67b8-4bb0-a33a-ecf99b651999: Object {"signal": Object {"candidate": String("candidate:2839139417 1 udp 2130706431 fd7a:115c:a1e0::3601:e412 55705 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:28:56.952628Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=3ff99b94-7d73-4928-b6ac-14d706034172 to=353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:28:56.952683Z DEBUG beach_road::websocket: Received WebSocket frame from peer 3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:28:56.952694Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:28:56.952702Z DEBUG beach_road::websocket: Text frame content from 3ff99b94-7d73-4928-b6ac-14d706034172: {"type":"signal","to_peer":"353847d9-67b8-4bb0-a33a-ecf99b651999","signal":{"signal":{"candidate":"candidate:2193730423 1 udp 2130706431 100.88.228.18 62699 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-29T00:28:56.952739Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "353847d9-67b8-4bb0-a33a-ecf99b651999", signal: Object {"signal": Object {"candidate": String("candidate:2193730423 1 udp 2130706431 100.88.228.18 62699 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:28:56.952767Z DEBUG beach_road::websocket: Received Signal from 3ff99b94-7d73-4928-b6ac-14d706034172 to 353847d9-67b8-4bb0-a33a-ecf99b651999: Object {"signal": Object {"candidate": String("candidate:2193730423 1 udp 2130706431 100.88.228.18 62699 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:28:56.952840Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=3ff99b94-7d73-4928-b6ac-14d706034172 to=353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:28:56.952875Z DEBUG beach_road::websocket: Received WebSocket frame from peer 3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:28:56.952883Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:28:56.952891Z DEBUG beach_road::websocket: Text frame content from 3ff99b94-7d73-4928-b6ac-14d706034172: {"type":"signal","to_peer":"353847d9-67b8-4bb0-a33a-ecf99b651999","signal":{"signal":{"candidate":"candidate:2323497841 1 udp 1694498815 72.227.131.114 58125 typ srflx raddr 0.0.0.0 rport 58125","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-29T00:28:56.952965Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "353847d9-67b8-4bb0-a33a-ecf99b651999", signal: Object {"signal": Object {"candidate": String("candidate:2323497841 1 udp 1694498815 72.227.131.114 58125 typ srflx raddr 0.0.0.0 rport 58125"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:28:56.952990Z DEBUG beach_road::websocket: Received Signal from 3ff99b94-7d73-4928-b6ac-14d706034172 to 353847d9-67b8-4bb0-a33a-ecf99b651999: Object {"signal": Object {"candidate": String("candidate:2323497841 1 udp 1694498815 72.227.131.114 58125 typ srflx raddr 0.0.0.0 rport 58125"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:28:56.953020Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=3ff99b94-7d73-4928-b6ac-14d706034172 to=353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:28:56.953441Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=404
2025-09-29T00:28:56.956039Z DEBUG request{method=POST uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:28:56.956298Z DEBUG beach_road::websocket: Received WebSocket frame from peer 353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:28:56.956312Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:28:56.956321Z DEBUG beach_road::websocket: Text frame content from 353847d9-67b8-4bb0-a33a-ecf99b651999: {"type":"signal","to_peer":"3ff99b94-7d73-4928-b6ac-14d706034172","signal":{"signal":{"candidate":"candidate:3173101093 1 udp 2130706431 192.168.68.52 58791 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-29T00:28:56.956370Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "3ff99b94-7d73-4928-b6ac-14d706034172", signal: Object {"signal": Object {"candidate": String("candidate:3173101093 1 udp 2130706431 192.168.68.52 58791 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:28:56.956424Z DEBUG beach_road::websocket: Received Signal from 353847d9-67b8-4bb0-a33a-ecf99b651999 to 3ff99b94-7d73-4928-b6ac-14d706034172: Object {"signal": Object {"candidate": String("candidate:3173101093 1 udp 2130706431 192.168.68.52 58791 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:28:56.956480Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=353847d9-67b8-4bb0-a33a-ecf99b651999 to=3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:28:56.956536Z DEBUG beach_road::websocket: Received WebSocket frame from peer 353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:28:56.956548Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:28:56.956556Z DEBUG beach_road::websocket: Text frame content from 353847d9-67b8-4bb0-a33a-ecf99b651999: {"type":"signal","to_peer":"3ff99b94-7d73-4928-b6ac-14d706034172","signal":{"signal":{"candidate":"candidate:2193730423 1 udp 2130706431 100.88.228.18 61767 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-29T00:28:56.956594Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "3ff99b94-7d73-4928-b6ac-14d706034172", signal: Object {"signal": Object {"candidate": String("candidate:2193730423 1 udp 2130706431 100.88.228.18 61767 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:28:56.956616Z DEBUG beach_road::websocket: Received Signal from 353847d9-67b8-4bb0-a33a-ecf99b651999 to 3ff99b94-7d73-4928-b6ac-14d706034172: Object {"signal": Object {"candidate": String("candidate:2193730423 1 udp 2130706431 100.88.228.18 61767 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:28:56.956646Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=353847d9-67b8-4bb0-a33a-ecf99b651999 to=3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:28:56.956672Z DEBUG beach_road::websocket: Received WebSocket frame from peer 353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:28:56.956679Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:28:56.956746Z DEBUG beach_road::websocket: Text frame content from 353847d9-67b8-4bb0-a33a-ecf99b651999: {"type":"signal","to_peer":"3ff99b94-7d73-4928-b6ac-14d706034172","signal":{"signal":{"candidate":"candidate:2839139417 1 udp 2130706431 fd7a:115c:a1e0::3601:e412 55460 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-29T00:28:56.956785Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "3ff99b94-7d73-4928-b6ac-14d706034172", signal: Object {"signal": Object {"candidate": String("candidate:2839139417 1 udp 2130706431 fd7a:115c:a1e0::3601:e412 55460 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:28:56.956822Z DEBUG beach_road::websocket: Received Signal from 353847d9-67b8-4bb0-a33a-ecf99b651999 to 3ff99b94-7d73-4928-b6ac-14d706034172: Object {"signal": Object {"candidate": String("candidate:2839139417 1 udp 2130706431 fd7a:115c:a1e0::3601:e412 55460 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:28:56.956886Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=353847d9-67b8-4bb0-a33a-ecf99b651999 to=3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:28:56.959657Z DEBUG request{method=POST uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=204
2025-09-29T00:28:56.991031Z DEBUG beach_road::websocket: Received WebSocket frame from peer 353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:28:56.991062Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:28:56.991071Z DEBUG beach_road::websocket: Text frame content from 353847d9-67b8-4bb0-a33a-ecf99b651999: {"type":"signal","to_peer":"3ff99b94-7d73-4928-b6ac-14d706034172","signal":{"signal":{"candidate":"candidate:2323497841 1 udp 1694498815 72.227.131.114 65022 typ srflx raddr 0.0.0.0 rport 65022","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-29T00:28:56.991132Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "3ff99b94-7d73-4928-b6ac-14d706034172", signal: Object {"signal": Object {"candidate": String("candidate:2323497841 1 udp 1694498815 72.227.131.114 65022 typ srflx raddr 0.0.0.0 rport 65022"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:28:56.991163Z DEBUG beach_road::websocket: Received Signal from 353847d9-67b8-4bb0-a33a-ecf99b651999 to 3ff99b94-7d73-4928-b6ac-14d706034172: Object {"signal": Object {"candidate": String("candidate:2323497841 1 udp 1694498815 72.227.131.114 65022 typ srflx raddr 0.0.0.0 rport 65022"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:28:56.991210Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=353847d9-67b8-4bb0-a33a-ecf99b651999 to=3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:28:57.210536Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:28:57.216564Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=200
2025-09-29T00:28:57.308418Z DEBUG request{method=GET uri=/ws/37947890-7cc8-41e0-a1aa-007377847a99 version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:28:57.308543Z DEBUG request{method=GET uri=/ws/37947890-7cc8-41e0-a1aa-007377847a99 version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=0 ms status=101
2025-09-29T00:28:57.308652Z DEBUG beach_road::websocket: WebSocket connected: peer=93d239af-739e-41bd-a047-c2588aacf488 session=37947890-7cc8-41e0-a1aa-007377847a99
2025-09-29T00:28:57.308993Z DEBUG beach_road::websocket: Received WebSocket frame from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:28:57.309014Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:28:57.309022Z DEBUG beach_road::websocket: Text frame content from 93d239af-739e-41bd-a047-c2588aacf488: {"type":"join","peer_id":"b0b7520c-9b9e-45fe-9c06-8ef122d6e0ea","passphrase":"204220","supported_transports":["webrtc"],"preferred_transport":"webrtc"}
2025-09-29T00:28:57.309067Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Join { peer_id: "b0b7520c-9b9e-45fe-9c06-8ef122d6e0ea", passphrase: Some("204220"), supported_transports: [WebRTC], preferred_transport: Some(WebRTC) }
2025-09-29T00:28:57.309087Z  INFO beach_road::websocket: 📥 RECEIVED Join message from peer 93d239af-739e-41bd-a047-c2588aacf488 (client_peer_id: "b0b7520c-9b9e-45fe-9c06-8ef122d6e0ea") for session 37947890-7cc8-41e0-a1aa-007377847a99
2025-09-29T00:28:57.309132Z  INFO beach_road::websocket:   → Session has existing peers, assigning role: Client
2025-09-29T00:28:57.309149Z  INFO beach_road::websocket:   → Added peer 93d239af-739e-41bd-a047-c2588aacf488 to session 37947890-7cc8-41e0-a1aa-007377847a99 with role Client
2025-09-29T00:28:57.309197Z  INFO beach_road::websocket:   → Session now has 3 peers, available transports: [WebRTC]
2025-09-29T00:28:57.309226Z  INFO beach_road::websocket: 📤 SENDING JoinSuccess to peer 93d239af-739e-41bd-a047-c2588aacf488: session=37947890-7cc8-41e0-a1aa-007377847a99, peer_id=93d239af-739e-41bd-a047-c2588aacf488, peers=3, transports=[WebRTC]
2025-09-29T00:28:57.309239Z  INFO beach_road::websocket:   → JoinSuccess sent successfully to peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:28:57.309566Z DEBUG beach_road::websocket: Received WebSocket frame from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:28:57.309579Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:28:57.309587Z DEBUG beach_road::websocket: Text frame content from 93d239af-739e-41bd-a047-c2588aacf488: {"type":"ping"}
2025-09-29T00:28:57.309598Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Ping
2025-09-29T00:28:57.311396Z DEBUG request{method=POST uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/offer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:28:57.311598Z DEBUG beach_road::websocket: Received WebSocket frame from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:28:57.311611Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:28:57.311618Z DEBUG beach_road::websocket: Text frame content from 93d239af-739e-41bd-a047-c2588aacf488: {"type":"signal","to_peer":"353847d9-67b8-4bb0-a33a-ecf99b651999","signal":{"signal":{"candidate":"candidate:2839139417 1 udp 2130706431 fd7a:115c:a1e0::3601:e412 61517 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-29T00:28:57.311658Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "353847d9-67b8-4bb0-a33a-ecf99b651999", signal: Object {"signal": Object {"candidate": String("candidate:2839139417 1 udp 2130706431 fd7a:115c:a1e0::3601:e412 61517 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:28:57.311686Z DEBUG beach_road::websocket: Received Signal from 93d239af-739e-41bd-a047-c2588aacf488 to 353847d9-67b8-4bb0-a33a-ecf99b651999: Object {"signal": Object {"candidate": String("candidate:2839139417 1 udp 2130706431 fd7a:115c:a1e0::3601:e412 61517 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:28:57.311726Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=93d239af-739e-41bd-a047-c2588aacf488 to=353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:28:57.311762Z DEBUG beach_road::websocket: Received WebSocket frame from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:28:57.311770Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:28:57.311777Z DEBUG beach_road::websocket: Text frame content from 93d239af-739e-41bd-a047-c2588aacf488: {"type":"signal","to_peer":"353847d9-67b8-4bb0-a33a-ecf99b651999","signal":{"signal":{"candidate":"candidate:2193730423 1 udp 2130706431 100.88.228.18 55044 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-29T00:28:57.311803Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "353847d9-67b8-4bb0-a33a-ecf99b651999", signal: Object {"signal": Object {"candidate": String("candidate:2193730423 1 udp 2130706431 100.88.228.18 55044 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:28:57.311821Z DEBUG beach_road::websocket: Received Signal from 93d239af-739e-41bd-a047-c2588aacf488 to 353847d9-67b8-4bb0-a33a-ecf99b651999: Object {"signal": Object {"candidate": String("candidate:2193730423 1 udp 2130706431 100.88.228.18 55044 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:28:57.311932Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=93d239af-739e-41bd-a047-c2588aacf488 to=353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:28:57.311976Z DEBUG beach_road::websocket: Received WebSocket frame from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:28:57.311984Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:28:57.311991Z DEBUG beach_road::websocket: Text frame content from 93d239af-739e-41bd-a047-c2588aacf488: {"type":"signal","to_peer":"353847d9-67b8-4bb0-a33a-ecf99b651999","signal":{"signal":{"candidate":"candidate:3173101093 1 udp 2130706431 192.168.68.52 61426 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-29T00:28:57.312027Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "353847d9-67b8-4bb0-a33a-ecf99b651999", signal: Object {"signal": Object {"candidate": String("candidate:3173101093 1 udp 2130706431 192.168.68.52 61426 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:28:57.312048Z DEBUG beach_road::websocket: Received Signal from 93d239af-739e-41bd-a047-c2588aacf488 to 353847d9-67b8-4bb0-a33a-ecf99b651999: Object {"signal": Object {"candidate": String("candidate:3173101093 1 udp 2130706431 192.168.68.52 61426 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:28:57.312073Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=93d239af-739e-41bd-a047-c2588aacf488 to=353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:28:57.314896Z DEBUG request{method=POST uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/offer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=204
2025-09-29T00:28:57.315396Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:28:57.315950Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=0 ms status=404
2025-09-29T00:28:57.341499Z DEBUG beach_road::websocket: Received WebSocket frame from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:28:57.341515Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:28:57.341522Z DEBUG beach_road::websocket: Text frame content from 93d239af-739e-41bd-a047-c2588aacf488: {"type":"signal","to_peer":"353847d9-67b8-4bb0-a33a-ecf99b651999","signal":{"signal":{"candidate":"candidate:2323497841 1 udp 1694498815 72.227.131.114 54501 typ srflx raddr 0.0.0.0 rport 54501","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-29T00:28:57.341573Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "353847d9-67b8-4bb0-a33a-ecf99b651999", signal: Object {"signal": Object {"candidate": String("candidate:2323497841 1 udp 1694498815 72.227.131.114 54501 typ srflx raddr 0.0.0.0 rport 54501"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:28:57.341597Z DEBUG beach_road::websocket: Received Signal from 93d239af-739e-41bd-a047-c2588aacf488 to 353847d9-67b8-4bb0-a33a-ecf99b651999: Object {"signal": Object {"candidate": String("candidate:2323497841 1 udp 1694498815 72.227.131.114 54501 typ srflx raddr 0.0.0.0 rport 54501"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:28:57.341640Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=93d239af-739e-41bd-a047-c2588aacf488 to=353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:28:57.571156Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:28:57.575477Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:28:57.832160Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:28:57.837108Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:28:58.093465Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:28:58.096665Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:28:58.352413Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:28:58.357233Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:28:58.619344Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:28:58.623231Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:28:58.877656Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:28:58.881299Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:28:59.138545Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:28:59.144153Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-29T00:28:59.397833Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:28:59.401924Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:28:59.657665Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:28:59.660280Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-29T00:28:59.915354Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:28:59.919048Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:00.173711Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:00.178144Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:00.432518Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:00.433993Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=404
2025-09-29T00:29:00.686141Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:00.689211Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:00.942184Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:00.947009Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:01.200836Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:01.204190Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:01.459438Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:01.462410Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:01.717730Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:01.723161Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:01.977949Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:01.982878Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:02.235872Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:02.239600Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:02.495392Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:02.499069Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:02.752880Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:02.756044Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:03.010639Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:03.015891Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:03.270671Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:03.276402Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:03.530419Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:03.531273Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=0 ms status=404
2025-09-29T00:29:03.785604Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:03.789918Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:04.046495Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:04.049360Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:04.304199Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:04.308164Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:04.566158Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:04.573181Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-29T00:29:04.829120Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:04.831322Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-29T00:29:05.086350Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:05.091809Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:05.348992Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:05.353112Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:05.611793Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:05.617007Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:05.870147Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:05.874905Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:06.128959Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:06.132170Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:06.388062Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:06.391114Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:06.648537Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:06.653506Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:06.908675Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:06.911438Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-29T00:29:07.166169Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:07.172042Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:07.428535Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:07.431297Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-29T00:29:07.686953Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:07.689451Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-29T00:29:07.944242Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:07.947524Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:08.201149Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:08.209748Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=8 ms status=404
2025-09-29T00:29:08.465713Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:08.468781Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:08.724382Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:08.729834Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:08.986273Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:08.995397Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=9 ms status=404
2025-09-29T00:29:09.248927Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:09.251871Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:09.507648Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:09.514974Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-29T00:29:09.770703Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:09.776578Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:10.039545Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:10.050672Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=11 ms status=404
2025-09-29T00:29:10.315246Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:10.319108Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:10.577385Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:10.581447Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:10.837159Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:10.843538Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:11.106058Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:11.111949Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:11.365848Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:11.370115Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:11.625775Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:11.630418Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:11.887733Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:11.894012Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:12.148756Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:12.150311Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=404
2025-09-29T00:29:12.405049Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:12.408238Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:12.665329Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:12.669879Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:12.928679Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:12.931664Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:13.187807Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:13.194739Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-29T00:29:13.451345Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:13.455282Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:13.712714Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:13.720071Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-29T00:29:13.978908Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:13.983890Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:14.239747Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:14.244541Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:14.503275Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:14.509201Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:14.782098Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:14.784732Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-29T00:29:14.785599Z DEBUG request{method=OPTIONS uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/join version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:14.785662Z DEBUG request{method=OPTIONS uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/join version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=0 ms status=200
2025-09-29T00:29:14.787131Z DEBUG request{method=POST uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/join version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:14.787257Z DEBUG request{method=POST uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/join version=HTTP/1.1}: beach_road::handlers: Client attempting to join session: 37947890-7cc8-41e0-a1aa-007377847a99
2025-09-29T00:29:14.789613Z DEBUG request{method=POST uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/join version=HTTP/1.1}: beach_road::handlers: Client successfully joined session: 37947890-7cc8-41e0-a1aa-007377847a99
2025-09-29T00:29:14.789762Z DEBUG request{method=POST uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/join version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=200
2025-09-29T00:29:14.792743Z DEBUG request{method=GET uri=/ws/37947890-7cc8-41e0-a1aa-007377847a99 version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:14.792853Z DEBUG request{method=GET uri=/ws/37947890-7cc8-41e0-a1aa-007377847a99 version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=0 ms status=101
2025-09-29T00:29:14.792978Z DEBUG beach_road::websocket: WebSocket connected: peer=ddc11e3a-0600-4319-ad59-edc8f9d5d914 session=37947890-7cc8-41e0-a1aa-007377847a99
2025-09-29T00:29:14.794112Z DEBUG beach_road::websocket: Received WebSocket frame from peer ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:14.794148Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:14.794155Z DEBUG beach_road::websocket: Text frame content from ddc11e3a-0600-4319-ad59-edc8f9d5d914: {"type":"join","peer_id":"ea0fad03-92a6-401c-8491-84af56eb0a0e","passphrase":"204220","supported_transports":["webrtc"]}
2025-09-29T00:29:14.794221Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Join { peer_id: "ea0fad03-92a6-401c-8491-84af56eb0a0e", passphrase: Some("204220"), supported_transports: [WebRTC], preferred_transport: None }
2025-09-29T00:29:14.794244Z  INFO beach_road::websocket: 📥 RECEIVED Join message from peer ddc11e3a-0600-4319-ad59-edc8f9d5d914 (client_peer_id: "ea0fad03-92a6-401c-8491-84af56eb0a0e") for session 37947890-7cc8-41e0-a1aa-007377847a99
2025-09-29T00:29:14.794312Z  INFO beach_road::websocket:   → Session has existing peers, assigning role: Client
2025-09-29T00:29:14.794340Z  INFO beach_road::websocket:   → Added peer ddc11e3a-0600-4319-ad59-edc8f9d5d914 to session 37947890-7cc8-41e0-a1aa-007377847a99 with role Client
2025-09-29T00:29:14.794410Z  INFO beach_road::websocket:   → Session now has 4 peers, available transports: [WebRTC]
2025-09-29T00:29:14.794440Z  INFO beach_road::websocket: 📤 SENDING JoinSuccess to peer ddc11e3a-0600-4319-ad59-edc8f9d5d914: session=37947890-7cc8-41e0-a1aa-007377847a99, peer_id=ddc11e3a-0600-4319-ad59-edc8f9d5d914, peers=4, transports=[WebRTC]
2025-09-29T00:29:14.794457Z  INFO beach_road::websocket:   → JoinSuccess sent successfully to peer ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:14.795562Z DEBUG beach_road::websocket: Received WebSocket frame from peer ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:14.795577Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:14.795585Z DEBUG beach_road::websocket: Text frame content from ddc11e3a-0600-4319-ad59-edc8f9d5d914: {"type":"negotiate_transport","to_peer":"3ff99b94-7d73-4928-b6ac-14d706034172","proposed_transport":"webrtc"}
2025-09-29T00:29:14.795647Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: NegotiateTransport { to_peer: "3ff99b94-7d73-4928-b6ac-14d706034172", proposed_transport: WebRTC }
2025-09-29T00:29:14.799641Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/offer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:14.800857Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/offer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=200
2025-09-29T00:29:14.804107Z DEBUG request{method=OPTIONS uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:14.804276Z DEBUG request{method=OPTIONS uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=0 ms status=200
2025-09-29T00:29:14.805511Z DEBUG request{method=POST uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:14.807819Z DEBUG request{method=POST uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=204
2025-09-29T00:29:15.047312Z DEBUG request{method=POST uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/offer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:15.057090Z DEBUG beach_road::websocket: Received WebSocket frame from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:29:15.057160Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:29:15.057183Z DEBUG beach_road::websocket: Text frame content from 93d239af-739e-41bd-a047-c2588aacf488: {"type":"signal","to_peer":"ddc11e3a-0600-4319-ad59-edc8f9d5d914","signal":{"signal":{"candidate":"candidate:2193730423 1 udp 2130706431 100.88.228.18 56916 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-29T00:29:15.057351Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "ddc11e3a-0600-4319-ad59-edc8f9d5d914", signal: Object {"signal": Object {"candidate": String("candidate:2193730423 1 udp 2130706431 100.88.228.18 56916 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:29:15.060694Z DEBUG beach_road::websocket: Received Signal from 93d239af-739e-41bd-a047-c2588aacf488 to ddc11e3a-0600-4319-ad59-edc8f9d5d914: Object {"signal": Object {"candidate": String("candidate:2193730423 1 udp 2130706431 100.88.228.18 56916 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:29:15.060929Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=93d239af-739e-41bd-a047-c2588aacf488 to=ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:15.061069Z DEBUG beach_road::websocket: Received WebSocket frame from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:29:15.061089Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:29:15.061257Z DEBUG beach_road::websocket: Text frame content from 93d239af-739e-41bd-a047-c2588aacf488: {"type":"signal","to_peer":"ddc11e3a-0600-4319-ad59-edc8f9d5d914","signal":{"signal":{"candidate":"candidate:2839139417 1 udp 2130706431 fd7a:115c:a1e0::3601:e412 61891 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-29T00:29:15.061722Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "ddc11e3a-0600-4319-ad59-edc8f9d5d914", signal: Object {"signal": Object {"candidate": String("candidate:2839139417 1 udp 2130706431 fd7a:115c:a1e0::3601:e412 61891 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:29:15.061785Z DEBUG beach_road::websocket: Received Signal from 93d239af-739e-41bd-a047-c2588aacf488 to ddc11e3a-0600-4319-ad59-edc8f9d5d914: Object {"signal": Object {"candidate": String("candidate:2839139417 1 udp 2130706431 fd7a:115c:a1e0::3601:e412 61891 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:29:15.060557Z DEBUG request{method=POST uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/offer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=13 ms status=204
2025-09-29T00:29:15.062128Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=93d239af-739e-41bd-a047-c2588aacf488 to=ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:15.062278Z DEBUG beach_road::websocket: Received WebSocket frame from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:29:15.062301Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:29:15.062321Z DEBUG beach_road::websocket: Text frame content from 93d239af-739e-41bd-a047-c2588aacf488: {"type":"signal","to_peer":"ddc11e3a-0600-4319-ad59-edc8f9d5d914","signal":{"signal":{"candidate":"candidate:3173101093 1 udp 2130706431 192.168.68.52 59052 typ host","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-29T00:29:15.062564Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "ddc11e3a-0600-4319-ad59-edc8f9d5d914", signal: Object {"signal": Object {"candidate": String("candidate:3173101093 1 udp 2130706431 192.168.68.52 59052 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:29:15.062646Z DEBUG beach_road::websocket: Received Signal from 93d239af-739e-41bd-a047-c2588aacf488 to ddc11e3a-0600-4319-ad59-edc8f9d5d914: Object {"signal": Object {"candidate": String("candidate:3173101093 1 udp 2130706431 192.168.68.52 59052 typ host"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:29:15.062730Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=93d239af-739e-41bd-a047-c2588aacf488 to=ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:15.063464Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:15.070037Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:15.104192Z DEBUG beach_road::websocket: Received WebSocket frame from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:29:15.104258Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:29:15.104275Z DEBUG beach_road::websocket: Text frame content from 93d239af-739e-41bd-a047-c2588aacf488: {"type":"signal","to_peer":"ddc11e3a-0600-4319-ad59-edc8f9d5d914","signal":{"signal":{"candidate":"candidate:2323497841 1 udp 1694498815 72.227.131.114 61132 typ srflx raddr 0.0.0.0 rport 61132","sdp_mid":"","sdp_mline_index":0,"signal_type":"ice_candidate"},"transport":"webrtc"}}
2025-09-29T00:29:15.104447Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "ddc11e3a-0600-4319-ad59-edc8f9d5d914", signal: Object {"signal": Object {"candidate": String("candidate:2323497841 1 udp 1694498815 72.227.131.114 61132 typ srflx raddr 0.0.0.0 rport 61132"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:29:15.104509Z DEBUG beach_road::websocket: Received Signal from 93d239af-739e-41bd-a047-c2588aacf488 to ddc11e3a-0600-4319-ad59-edc8f9d5d914: Object {"signal": Object {"candidate": String("candidate:2323497841 1 udp 1694498815 72.227.131.114 61132 typ srflx raddr 0.0.0.0 rport 61132"), "sdp_mid": String(""), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:29:15.104730Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=93d239af-739e-41bd-a047-c2588aacf488 to=ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:15.215402Z DEBUG beach_road::websocket: Received WebSocket frame from peer ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:15.215460Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:15.215480Z DEBUG beach_road::websocket: Text frame content from ddc11e3a-0600-4319-ad59-edc8f9d5d914: {"type":"signal","to_peer":"3ff99b94-7d73-4928-b6ac-14d706034172","signal":{"transport":"webrtc","signal":{"signal_type":"ice_candidate","candidate":"candidate:5606886 1 udp 2113937151 f48d1ff3-9ed3-45c2-a73f-a9f7ac97b2e4.local 62250 typ host generation 0 ufrag Q7sc network-cost 999","sdp_mid":"0","sdp_mline_index":0}}}
2025-09-29T00:29:15.215606Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "3ff99b94-7d73-4928-b6ac-14d706034172", signal: Object {"signal": Object {"candidate": String("candidate:5606886 1 udp 2113937151 f48d1ff3-9ed3-45c2-a73f-a9f7ac97b2e4.local 62250 typ host generation 0 ufrag Q7sc network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:29:15.215723Z DEBUG beach_road::websocket: Received Signal from ddc11e3a-0600-4319-ad59-edc8f9d5d914 to 3ff99b94-7d73-4928-b6ac-14d706034172: Object {"signal": Object {"candidate": String("candidate:5606886 1 udp 2113937151 f48d1ff3-9ed3-45c2-a73f-a9f7ac97b2e4.local 62250 typ host generation 0 ufrag Q7sc network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:29:15.215912Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=ddc11e3a-0600-4319-ad59-edc8f9d5d914 to=3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:29:15.216003Z DEBUG beach_road::websocket: Received WebSocket frame from peer ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:15.216023Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:15.216043Z DEBUG beach_road::websocket: Text frame content from ddc11e3a-0600-4319-ad59-edc8f9d5d914: {"type":"signal","to_peer":"3ff99b94-7d73-4928-b6ac-14d706034172","signal":{"transport":"webrtc","signal":{"signal_type":"ice_candidate","candidate":"candidate:3656812828 1 udp 1677729535 72.227.131.114 62250 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag Q7sc network-cost 999","sdp_mid":"0","sdp_mline_index":0}}}
2025-09-29T00:29:15.216123Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "3ff99b94-7d73-4928-b6ac-14d706034172", signal: Object {"signal": Object {"candidate": String("candidate:3656812828 1 udp 1677729535 72.227.131.114 62250 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag Q7sc network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:29:15.216187Z DEBUG beach_road::websocket: Received Signal from ddc11e3a-0600-4319-ad59-edc8f9d5d914 to 3ff99b94-7d73-4928-b6ac-14d706034172: Object {"signal": Object {"candidate": String("candidate:3656812828 1 udp 1677729535 72.227.131.114 62250 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag Q7sc network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:29:15.216293Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=ddc11e3a-0600-4319-ad59-edc8f9d5d914 to=3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:29:15.269084Z DEBUG beach_road::websocket: Received WebSocket frame from peer 3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:29:15.269158Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:29:15.269182Z DEBUG beach_road::websocket: Text frame content from 3ff99b94-7d73-4928-b6ac-14d706034172: {"type":"ping"}
2025-09-29T00:29:15.269231Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Ping
2025-09-29T00:29:15.334647Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:15.338302Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=10 ms status=404
2025-09-29T00:29:15.594305Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:15.600307Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:15.854958Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:15.868676Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=13 ms status=404
2025-09-29T00:29:16.124026Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:16.128818Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:16.386382Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:16.391818Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:16.426347Z DEBUG beach_road::websocket: Received WebSocket frame from peer ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:16.426446Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:16.426470Z DEBUG beach_road::websocket: Text frame content from ddc11e3a-0600-4319-ad59-edc8f9d5d914: {"type":"signal","to_peer":"3ff99b94-7d73-4928-b6ac-14d706034172","signal":{"transport":"webrtc","signal":{"signal_type":"ice_candidate","candidate":"candidate:5606886 1 udp 2113937151 f48d1ff3-9ed3-45c2-a73f-a9f7ac97b2e4.local 62250 typ host generation 0 ufrag Q7sc network-cost 999","sdp_mid":"0","sdp_mline_index":0}}}
2025-09-29T00:29:16.426676Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "3ff99b94-7d73-4928-b6ac-14d706034172", signal: Object {"signal": Object {"candidate": String("candidate:5606886 1 udp 2113937151 f48d1ff3-9ed3-45c2-a73f-a9f7ac97b2e4.local 62250 typ host generation 0 ufrag Q7sc network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:29:16.427415Z DEBUG beach_road::websocket: Received Signal from ddc11e3a-0600-4319-ad59-edc8f9d5d914 to 3ff99b94-7d73-4928-b6ac-14d706034172: Object {"signal": Object {"candidate": String("candidate:5606886 1 udp 2113937151 f48d1ff3-9ed3-45c2-a73f-a9f7ac97b2e4.local 62250 typ host generation 0 ufrag Q7sc network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:29:16.427593Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=ddc11e3a-0600-4319-ad59-edc8f9d5d914 to=3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:29:16.446509Z DEBUG beach_road::websocket: Received WebSocket frame from peer ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:16.446554Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:16.446569Z DEBUG beach_road::websocket: Text frame content from ddc11e3a-0600-4319-ad59-edc8f9d5d914: {"type":"signal","to_peer":"3ff99b94-7d73-4928-b6ac-14d706034172","signal":{"transport":"webrtc","signal":{"signal_type":"ice_candidate","candidate":"candidate:3656812828 1 udp 1677729535 72.227.131.114 62250 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag Q7sc network-cost 999","sdp_mid":"0","sdp_mline_index":0}}}
2025-09-29T00:29:16.446676Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "3ff99b94-7d73-4928-b6ac-14d706034172", signal: Object {"signal": Object {"candidate": String("candidate:3656812828 1 udp 1677729535 72.227.131.114 62250 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag Q7sc network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:29:16.446724Z DEBUG beach_road::websocket: Received Signal from ddc11e3a-0600-4319-ad59-edc8f9d5d914 to 3ff99b94-7d73-4928-b6ac-14d706034172: Object {"signal": Object {"candidate": String("candidate:3656812828 1 udp 1677729535 72.227.131.114 62250 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag Q7sc network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:29:16.446805Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=ddc11e3a-0600-4319-ad59-edc8f9d5d914 to=3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:29:16.649135Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:16.654756Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:16.911651Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:16.918637Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-29T00:29:17.174425Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:17.180712Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:17.438971Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:17.444284Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-29T00:29:17.631864Z DEBUG beach_road::websocket: Received WebSocket frame from peer ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:17.631993Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:17.632016Z DEBUG beach_road::websocket: Text frame content from ddc11e3a-0600-4319-ad59-edc8f9d5d914: {"type":"signal","to_peer":"3ff99b94-7d73-4928-b6ac-14d706034172","signal":{"transport":"webrtc","signal":{"signal_type":"ice_candidate","candidate":"candidate:5606886 1 udp 2113937151 f48d1ff3-9ed3-45c2-a73f-a9f7ac97b2e4.local 62250 typ host generation 0 ufrag Q7sc network-cost 999","sdp_mid":"0","sdp_mline_index":0}}}
2025-09-29T00:29:17.632209Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "3ff99b94-7d73-4928-b6ac-14d706034172", signal: Object {"signal": Object {"candidate": String("candidate:5606886 1 udp 2113937151 f48d1ff3-9ed3-45c2-a73f-a9f7ac97b2e4.local 62250 typ host generation 0 ufrag Q7sc network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:29:17.632305Z DEBUG beach_road::websocket: Received Signal from ddc11e3a-0600-4319-ad59-edc8f9d5d914 to 3ff99b94-7d73-4928-b6ac-14d706034172: Object {"signal": Object {"candidate": String("candidate:5606886 1 udp 2113937151 f48d1ff3-9ed3-45c2-a73f-a9f7ac97b2e4.local 62250 typ host generation 0 ufrag Q7sc network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:29:17.632731Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=ddc11e3a-0600-4319-ad59-edc8f9d5d914 to=3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:29:17.632882Z DEBUG beach_road::websocket: Received WebSocket frame from peer ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:17.632901Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:17.632919Z DEBUG beach_road::websocket: Text frame content from ddc11e3a-0600-4319-ad59-edc8f9d5d914: {"type":"signal","to_peer":"3ff99b94-7d73-4928-b6ac-14d706034172","signal":{"transport":"webrtc","signal":{"signal_type":"ice_candidate","candidate":"candidate:3656812828 1 udp 1677729535 72.227.131.114 62250 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag Q7sc network-cost 999","sdp_mid":"0","sdp_mline_index":0}}}
2025-09-29T00:29:17.633072Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "3ff99b94-7d73-4928-b6ac-14d706034172", signal: Object {"signal": Object {"candidate": String("candidate:3656812828 1 udp 1677729535 72.227.131.114 62250 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag Q7sc network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:29:17.633148Z DEBUG beach_road::websocket: Received Signal from ddc11e3a-0600-4319-ad59-edc8f9d5d914 to 3ff99b94-7d73-4928-b6ac-14d706034172: Object {"signal": Object {"candidate": String("candidate:3656812828 1 udp 1677729535 72.227.131.114 62250 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag Q7sc network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:29:17.633222Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=ddc11e3a-0600-4319-ad59-edc8f9d5d914 to=3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:29:17.697086Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:17.698693Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=404
2025-09-29T00:29:17.955337Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:17.959809Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:18.219792Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:18.226009Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:18.481005Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:18.485466Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:18.742873Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:18.747459Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:18.839034Z DEBUG beach_road::websocket: Received WebSocket frame from peer ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:18.839171Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:18.839195Z DEBUG beach_road::websocket: Text frame content from ddc11e3a-0600-4319-ad59-edc8f9d5d914: {"type":"signal","to_peer":"3ff99b94-7d73-4928-b6ac-14d706034172","signal":{"transport":"webrtc","signal":{"signal_type":"ice_candidate","candidate":"candidate:5606886 1 udp 2113937151 f48d1ff3-9ed3-45c2-a73f-a9f7ac97b2e4.local 62250 typ host generation 0 ufrag Q7sc network-cost 999","sdp_mid":"0","sdp_mline_index":0}}}
2025-09-29T00:29:18.839412Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "3ff99b94-7d73-4928-b6ac-14d706034172", signal: Object {"signal": Object {"candidate": String("candidate:5606886 1 udp 2113937151 f48d1ff3-9ed3-45c2-a73f-a9f7ac97b2e4.local 62250 typ host generation 0 ufrag Q7sc network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:29:18.840354Z DEBUG beach_road::websocket: Received Signal from ddc11e3a-0600-4319-ad59-edc8f9d5d914 to 3ff99b94-7d73-4928-b6ac-14d706034172: Object {"signal": Object {"candidate": String("candidate:5606886 1 udp 2113937151 f48d1ff3-9ed3-45c2-a73f-a9f7ac97b2e4.local 62250 typ host generation 0 ufrag Q7sc network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:29:18.840534Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=ddc11e3a-0600-4319-ad59-edc8f9d5d914 to=3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:29:18.841301Z DEBUG beach_road::websocket: Received WebSocket frame from peer ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:18.841324Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer ddc11e3a-0600-4319-ad59-edc8f9d5d914
2025-09-29T00:29:18.841713Z DEBUG beach_road::websocket: Text frame content from ddc11e3a-0600-4319-ad59-edc8f9d5d914: {"type":"signal","to_peer":"3ff99b94-7d73-4928-b6ac-14d706034172","signal":{"transport":"webrtc","signal":{"signal_type":"ice_candidate","candidate":"candidate:3656812828 1 udp 1677729535 72.227.131.114 62250 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag Q7sc network-cost 999","sdp_mid":"0","sdp_mline_index":0}}}
2025-09-29T00:29:18.841815Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Signal { to_peer: "3ff99b94-7d73-4928-b6ac-14d706034172", signal: Object {"signal": Object {"candidate": String("candidate:3656812828 1 udp 1677729535 72.227.131.114 62250 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag Q7sc network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")} }
2025-09-29T00:29:18.842015Z DEBUG beach_road::websocket: Received Signal from ddc11e3a-0600-4319-ad59-edc8f9d5d914 to 3ff99b94-7d73-4928-b6ac-14d706034172: Object {"signal": Object {"candidate": String("candidate:3656812828 1 udp 1677729535 72.227.131.114 62250 typ srflx raddr 0.0.0.0 rport 0 generation 0 ufrag Q7sc network-cost 999"), "sdp_mid": String("0"), "sdp_mline_index": Number(0), "signal_type": String("ice_candidate")}, "transport": String("webrtc")}
2025-09-29T00:29:18.842119Z DEBUG beach_road::websocket: signal payload did not match TransportSignal target="beach_road::signal" from=ddc11e3a-0600-4319-ad59-edc8f9d5d914 to=3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:29:19.014484Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:19.023021Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=8 ms status=404
2025-09-29T00:29:19.280834Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:19.290209Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=9 ms status=404
2025-09-29T00:29:19.546355Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:19.549227Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:19.805648Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:19.807389Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=404
2025-09-29T00:29:20.062087Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:20.067141Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:20.320797Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:20.322576Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=404
2025-09-29T00:29:20.579238Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:20.582193Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:20.838791Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:20.844203Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:21.098742Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:21.104910Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:21.360982Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:21.365154Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:21.623052Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:21.628454Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:21.885099Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:21.890220Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:22.145376Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:22.151167Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:22.407051Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:22.409870Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-29T00:29:22.665068Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:22.671261Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:22.927660Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:22.930882Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:23.190104Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:23.193859Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:23.449272Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:23.455367Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:23.710643Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:23.715851Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:23.971026Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:23.974713Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:24.232741Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:24.237999Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:24.494487Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:24.498383Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:24.758965Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:24.762797Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:25.018078Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:25.022145Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:25.274940Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:25.277160Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-29T00:29:25.532578Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:25.538408Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:25.795816Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:25.800500Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:26.062102Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:26.065453Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:26.321284Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:26.327422Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:26.585142Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:26.590070Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:26.846509Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:26.853097Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:26.954917Z DEBUG beach_road::websocket: Received WebSocket frame from peer 353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:29:26.955102Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:29:26.955149Z DEBUG beach_road::websocket: Text frame content from 353847d9-67b8-4bb0-a33a-ecf99b651999: {"type":"ping"}
2025-09-29T00:29:26.955271Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Ping
2025-09-29T00:29:27.109094Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:27.115326Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-29T00:29:27.311213Z DEBUG beach_road::websocket: Received WebSocket frame from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:29:27.311394Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:29:27.311439Z DEBUG beach_road::websocket: Text frame content from 93d239af-739e-41bd-a047-c2588aacf488: {"type":"ping"}
2025-09-29T00:29:27.311553Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Ping
2025-09-29T00:29:27.370647Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:27.372608Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-29T00:29:27.628136Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:27.632260Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:27.887726Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:27.893236Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:28.149380Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:28.155361Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:28.411845Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:28.417510Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:28.673987Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:28.677949Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:28.934356Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:28.939677Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:29.196645Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:29.208904Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=13 ms status=404
2025-09-29T00:29:29.464977Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:29.469608Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:29.728091Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:29.732917Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:29.989647Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:29.994754Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:30.251029Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:30.255941Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:30.512289Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:30.516056Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:30.772705Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:30.777155Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:31.032239Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:31.038429Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:31.296059Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:31.300666Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:31.557073Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:31.561314Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:31.817296Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:31.822022Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:32.077389Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:32.085582Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=8 ms status=404
2025-09-29T00:29:32.340936Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:32.348021Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-29T00:29:32.603751Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:32.608581Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:32.864134Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:32.867622Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:33.123199Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:33.127532Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:33.389528Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:33.399924Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=10 ms status=404
2025-09-29T00:29:33.657465Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:33.666325Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=9 ms status=404
2025-09-29T00:29:33.923486Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:33.929847Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:34.185883Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:34.191215Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:34.445081Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:34.447374Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-29T00:29:34.703129Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:34.709022Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:34.965236Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:34.968794Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:35.223421Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:35.224799Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=404
2025-09-29T00:29:35.480334Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:35.492180Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=12 ms status=404
2025-09-29T00:29:35.748278Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:35.754833Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:36.011081Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:36.018185Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-29T00:29:36.273301Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:36.278643Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:36.534535Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:36.539141Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:36.794656Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:36.800103Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:37.056416Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:37.064960Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=8 ms status=404
2025-09-29T00:29:37.322405Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:37.327368Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:37.585038Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:37.589688Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:37.844176Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:37.851005Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-29T00:29:38.106312Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:38.114455Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=8 ms status=404
2025-09-29T00:29:38.369374Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:38.379473Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=10 ms status=404
2025-09-29T00:29:38.635529Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:38.640573Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:38.898327Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:38.903005Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:39.158171Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:39.160370Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:39.414091Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:39.418658Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:39.675680Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:39.679014Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:39.935693Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:39.940441Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:40.197993Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:40.203594Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:40.460845Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:40.466047Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:40.726098Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:40.730889Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:40.985336Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:40.991503Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:41.248853Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:41.255552Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:41.528898Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:41.531430Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-29T00:29:41.790732Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:41.797581Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-29T00:29:42.062749Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:42.065722Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:42.324765Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:42.327871Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:42.583553Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:42.589918Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-29T00:29:42.846077Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:42.851307Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:43.108178Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:43.115074Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-29T00:29:43.380540Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:43.388066Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-29T00:29:43.641814Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:43.643258Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=404
2025-09-29T00:29:43.898089Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:43.902365Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:44.160804Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:44.164006Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:44.418541Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:44.423871Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:44.679890Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:44.682539Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-29T00:29:44.938595Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:44.943149Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:45.196365Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:45.198284Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=404
2025-09-29T00:29:45.267404Z DEBUG beach_road::websocket: Received WebSocket frame from peer 3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:29:45.267441Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:29:45.267455Z DEBUG beach_road::websocket: Text frame content from 3ff99b94-7d73-4928-b6ac-14d706034172: {"type":"ping"}
2025-09-29T00:29:45.267484Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Ping
2025-09-29T00:29:45.456970Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:45.462747Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:45.718151Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:45.721474Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:45.977063Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:45.980248Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:46.235795Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:46.241590Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:46.495549Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:46.497928Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-29T00:29:46.752874Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:46.761422Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=8 ms status=404
2025-09-29T00:29:47.018641Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:47.022332Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:47.277665Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:47.284234Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:47.540338Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:47.544323Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:47.801213Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:47.805635Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:48.063136Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:48.068345Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:48.323464Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:48.329665Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:48.591213Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:48.595752Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:48.853835Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:48.860143Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:49.118568Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:49.127389Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=9 ms status=404
2025-09-29T00:29:49.386181Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:49.391943Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:49.647421Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:49.657045Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=9 ms status=404
2025-09-29T00:29:49.914985Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:49.918964Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:50.175185Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:50.179862Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:50.435659Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:50.442158Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:50.698573Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:50.704085Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:50.960451Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:50.964466Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:51.221262Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:51.226045Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:51.480897Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:51.484595Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:51.739040Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:51.744163Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:52.002728Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:52.006916Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:52.263888Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:52.268559Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:52.525692Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:52.530564Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:52.787017Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:52.791958Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:53.047005Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:53.051157Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:53.311085Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:53.316264Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:53.574739Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:53.575617Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=0 ms status=404
2025-09-29T00:29:53.830611Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:53.850178Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=19 ms status=404
2025-09-29T00:29:54.105709Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:54.125003Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=19 ms status=404
2025-09-29T00:29:54.380389Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:54.385433Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:54.641382Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:54.644440Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:54.900139Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:54.904491Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:55.160257Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:55.163328Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:55.418947Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:55.425335Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:55.680585Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:55.687023Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:29:55.946544Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:55.955281Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=8 ms status=404
2025-09-29T00:29:56.210453Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:56.217928Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-29T00:29:56.479544Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:56.484446Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:56.741760Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:56.748922Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-29T00:29:56.952739Z DEBUG beach_road::websocket: Received WebSocket frame from peer 353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:29:56.952892Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 353847d9-67b8-4bb0-a33a-ecf99b651999
2025-09-29T00:29:56.952936Z DEBUG beach_road::websocket: Text frame content from 353847d9-67b8-4bb0-a33a-ecf99b651999: {"type":"ping"}
2025-09-29T00:29:56.953014Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Ping
2025-09-29T00:29:57.005538Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:57.009483Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:57.264599Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:57.269064Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:57.310333Z DEBUG beach_road::websocket: Received WebSocket frame from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:29:57.310449Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 93d239af-739e-41bd-a047-c2588aacf488
2025-09-29T00:29:57.310482Z DEBUG beach_road::websocket: Text frame content from 93d239af-739e-41bd-a047-c2588aacf488: {"type":"ping"}
2025-09-29T00:29:57.310577Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Ping
2025-09-29T00:29:57.525789Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:57.530842Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:29:57.785868Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:57.789910Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:58.044648Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:58.048900Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:58.309541Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:58.314134Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:58.572282Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:58.575480Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:58.830723Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:58.835119Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:59.091869Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:59.096004Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:29:59.356412Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:59.359807Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:29:59.614775Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:59.627055Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=12 ms status=404
2025-09-29T00:29:59.888928Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:29:59.893082Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:00.151755Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:00.157614Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:30:00.414973Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:00.419353Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:00.674753Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:00.679441Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:00.938246Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:00.942990Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:01.197096Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:01.204983Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=8 ms status=404
2025-09-29T00:30:01.465287Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:01.469255Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:01.725345Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:01.730262Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:30:01.985628Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:01.989628Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:02.245929Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:02.250335Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:02.508992Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:02.515735Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-29T00:30:02.773423Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:02.781121Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=8 ms status=404
2025-09-29T00:30:03.039393Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:03.043812Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:03.301688Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:03.305611Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:03.559250Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:03.560214Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=404
2025-09-29T00:30:03.813129Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:03.819405Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:30:04.079462Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:04.083434Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:04.339282Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:04.353976Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=14 ms status=404
2025-09-29T00:30:04.611588Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:04.615826Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:04.873565Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:04.877063Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:30:05.134062Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:05.140090Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:30:05.397598Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:05.403361Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:30:05.659885Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:05.665320Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:30:05.920721Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:05.925025Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:06.185247Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:06.191121Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:30:06.452237Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:06.456520Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:06.714974Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:06.724076Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=9 ms status=404
2025-09-29T00:30:06.980441Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:06.984767Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:07.239272Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:07.243863Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:07.499775Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:07.503924Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:07.761036Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:07.765837Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:30:08.024638Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:08.028951Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:08.285364Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:08.291192Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:30:08.548067Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:08.560559Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=12 ms status=404
2025-09-29T00:30:08.821966Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:08.827274Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=12 ms status=404
2025-09-29T00:30:09.090667Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:09.095480Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:30:09.351619Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:09.356453Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:30:09.612078Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:09.616843Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:09.872448Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:09.879394Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-29T00:30:10.132729Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:10.135760Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:30:10.392289Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:10.395068Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-29T00:30:10.649030Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:10.650649Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=404
2025-09-29T00:30:10.908995Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:10.911202Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-29T00:30:11.167168Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:11.169783Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-29T00:30:11.426234Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:11.430436Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:11.686934Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:11.689660Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:30:11.942768Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:11.948757Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:30:12.205234Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:12.225505Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=20 ms status=404
2025-09-29T00:30:12.481416Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:12.484879Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:30:12.743094Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:12.747688Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:13.000887Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:13.003644Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=2 ms status=404
2025-09-29T00:30:13.258617Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:13.262383Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:13.516065Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:13.517565Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=1 ms status=404
2025-09-29T00:30:13.773265Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:13.776845Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:30:14.034685Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:14.037614Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:30:14.292818Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:14.297234Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:14.551665Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:14.556666Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:30:14.813374Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:14.825068Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=11 ms status=404
2025-09-29T00:30:15.085843Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:15.090164Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:15.266934Z DEBUG beach_road::websocket: Received WebSocket frame from peer 3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:30:15.267017Z DEBUG beach_road::websocket: Received WebSocket message type: "Text" from peer 3ff99b94-7d73-4928-b6ac-14d706034172
2025-09-29T00:30:15.267038Z DEBUG beach_road::websocket: Text frame content from 3ff99b94-7d73-4928-b6ac-14d706034172: {"type":"ping"}
2025-09-29T00:30:15.267076Z DEBUG beach_road::websocket: Successfully parsed ClientMessage from Text frame: Ping
2025-09-29T00:30:15.344505Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:15.348469Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:15.605825Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:15.611138Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:30:15.867601Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:15.872156Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:16.128487Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:16.134068Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:30:16.389013Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:16.392904Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:16.647821Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:16.651990Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:16.908264Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:16.925323Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=17 ms status=404
2025-09-29T00:30:17.180723Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:17.186775Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:30:17.443040Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:17.447214Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=4 ms status=404
2025-09-29T00:30:17.708349Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:17.716271Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=8 ms status=404
2025-09-29T00:30:17.972774Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:17.980480Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=7 ms status=404
2025-09-29T00:30:18.235983Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:18.242081Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:30:18.501025Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:18.504240Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=3 ms status=404
2025-09-29T00:30:18.757387Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:18.763921Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=6 ms status=404
2025-09-29T00:30:19.016963Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2025-09-29T00:30:19.021952Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=5 ms status=404
2025-09-29T00:30:19.280019Z DEBUG request{method=GET uri=/sessions/37947890-7cc8-41e0-a1aa-007377847a99/webrtc/answer version=HTTP/1.1}: tower_http::trace::on_request: started processing request
2```
# WebRTC Connection Issue Analysis

## Issue
WebRTC transport is created but never establishes a peer-to-peer connection between client and server.

## Root Cause
The WebRTC signaling exchange (SDP offer/answer and ICE candidates) is not implemented in the session layer.

### What's Working
1. ✅ WebSocket signaling connection establishes successfully
2. ✅ Subscription messages flow correctly through WebSocket 
3. ✅ Both peers (client and server) join the session
4. ✅ WebRTC transport is created with proper configuration

### What's Missing
1. ❌ No WebRTC offer is created when a client joins
2. ❌ No SDP offer/answer exchange through Signal messages
3. ❌ No ICE candidate exchange
4. ❌ The `connect_with_remote_signaling` method is never called

## Current Architecture

```
Client                    Session Server                    Server
  |                            |                               |
  |-- WebSocket Connect ------>|<------ WebSocket Connect -----|
  |                            |                               |
  |-- JoinSuccess ------------>|<---------- JoinSuccess -------|
  |                            |                               |
  |-- Subscribe (via Signal)-->|---> Subscribe (via Signal) -->|
  |                            |                               |
  |<- SubscriptionAck ---------|<--- SubscriptionAck -----------|
  |                            |                               |
  ❌ No WebRTC Offer           |          ❌ No handling        |
  ❌ No WebRTC Answer          |          ❌ No handling        |
  ❌ No ICE Candidates         |          ❌ No handling        |
```

## Required Implementation

### 1. Server Side (ServerSession)
When a client joins (`PeerJoined`), the server should:
1. Create a WebRTC offer using the transport
2. Send the offer via Signal message to the client
3. Wait for answer from client
4. Process ICE candidates

### 2. Client Side (ClientSession)  
When receiving an offer via Signal message:
1. Process the offer
2. Create an answer
3. Send answer back via Signal message
4. Exchange ICE candidates

### 3. Signal Message Handling
The Signal message handler in session/mod.rs needs to:
1. Detect WebRTC signaling messages (offer/answer/ice)
2. Pass them to the WebRTC transport for processing
3. Use `connect_with_remote_signaling` or similar mechanism

## Code Locations to Modify

1. `/apps/beach/src/session/mod.rs:132` - ServerSession Signal handler
   - Add WebRTC offer/answer/ICE handling
   
2. `/apps/beach/src/session/mod.rs:290` - ClientSession Signal handler (if exists)
   - Add WebRTC offer/answer/ICE handling

3. Create RemoteSignalingChannel implementation that bridges WebSocket Signal messages to WebRTC transport

## Next Steps

1. Implement RemoteSignalingChannel that uses WebSocket Signal messages
2. Initiate WebRTC connection when PeerJoined event occurs
3. Handle WebRTC signaling messages in Signal handler
4. Test peer-to-peer connection establishment
5. Verify data flows through WebRTC data channel instead of WebSocket
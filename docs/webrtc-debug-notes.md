# WebRTC Debug Notes

## Symptom
- Host and client negotiate WebRTC, host keeps sending snapshots/deltas, client times out and falls back to WebSocket.
- Host log shows WebRTC frames flowing; client log reports `TransportError::Timeout`.

## Hypothesis
- ICE negotiation never completes; we see repeated `pingAllCandidates called with no candidate pairs` warnings.
- Data channel open never fires; connection stuck in `connecting`.

## Tests
- Added `webrtc_bidirectional_transport_delivers_messages` using in-memory pair (passes).
- Added `webrtc_signaling_end_to_end` using real signaling server; reproduces timeout / warnings.
- Tests located at `apps/beach/tests/webrtc_transport.rs`.

## Next Steps
- Investigate ICE candidate exchange; ensure candidates exchanged via signaling.
- Confirm DTLS connection state transitions; add tracing around candidate gathering/setting.

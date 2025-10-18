# WebRTC bootstrap hang: diagnosis and fixes

This document captures the investigation into intermittent hangs during `beach ssh` bootstrap and proposes the right long‑term solution. The symptoms manifested in two phases:

- Phase A: Client stalled waiting for an SDP offer (WebRTC never established).
- Phase B: After transport connects, client UI stuck at “Connected – syncing remote session…”.

## Summary

- In Phase A, the Offerer (host) never posted an SDP offer, while the Answerer (client) polled `/offer` forever with 404. Primary root causes were signaling role/endpoint issues:
  - Host role misadvertised as `answerer` or missing → both sides became answerer.
  - Missing `PeerJoined(Client)` signal to the Offerer → Offerer never initiated negotiation.
  - REST path drift: server exposing `/fastpath/sessions/:id/webrtc/*` while clients posted/fetched `/sessions/:id/webrtc/*`.

- In Phase B, transport established (WebSocket fallback in this run), but the remote host process was torn down immediately after connect. The client stayed connected but never received Hello/grid snapshot, so the UI remained on “Connected – syncing…”. Root cause: SSH bootstrap ends the remote process after connect unless `--ssh-keep-host-running` is provided.

## Reproduction (as observed)

```
beach ssh ec2-user@<ip> \
  --ssh-flag=-i --ssh-flag=~/.ssh/key.pem \
  --ssh-flag=-o --ssh-flag=StrictHostKeyChecking=accept-new \
  --copy-binary --copy-from <workspace>/target/x86_64-unknown-linux-gnu/release/beach \
  --verify-binary-hash
```

Symptoms:
- Phase A: CLI prints session + join URL, then client logs show repeated
  `fetch_sdp.offer … result=Ok(404)` and no “offer posted” from host.
- Phase B: After connecting (often via WebSocket), UI shows “Connected – syncing remote session…” and never progresses.

## Key traces and what they mean

We added targeted logging in the Rust client/host to localize failures:

- Offerer (host):
  - `peer_joined` → Offerer received Client peer; should immediately create/post offer.
  - `offer created` (with `handshake_id`, `sdp_len`) → local SDP ready.
  - `post_sdp … url=… status=…` → offer POSTed; 200 indicates success.
  - `offer posted to signaling` → confirmation after successful POST.

- Answerer (client):
  - `fetch_sdp.offer … url=… status=…` → offer polling URL and status.
  - `offer applied` → remote description set; handshake proceeds.

Findings from runs:
- Client repeatedly logged `fetch_sdp.offer … Ok(404)`; host logs did not show `peer_joined` or `offer posted`. This indicates signaling never delivered `PeerJoined(Client)` to the host, and/or the POST endpoint was mismatched.
- In later runs, negotiation fell back to WebSocket and connected, but the host was killed immediately after connect, so the client never received Hello/backfill and remained on “syncing…”.

## Root causes

1) Signaling/role/endpoint issues
- Session service must advertise the host’s WebRTC offer with `role:"offerer"` and deliver `PeerJoined(Client)` events to the Offerer on WS join.
- Path mismatch: clients posted/fetched under `/sessions/:id/webrtc/*` while server expects `/fastpath/sessions/:id/webrtc/*` (or vice‑versa).

2) SSH bootstrap teardown too early
- Current `beach ssh` tears down the remote host right after transport connect unless `--ssh-keep-host-running` is passed. Transport connect != “sync complete”, so the UI can be left live but without a running host.

## Diagnostics added (in repo)

- Host (Offerer)
  - Log advertised WebRTC offer at startup (role, signaling URL).
  - Log `peer_joined`, `offer created`, `offer posted … url=… status=…`.

- Client (Answerer)
  - Log exact `fetch_sdp` URLs and statuses; log `offer applied`.

- Signaling client
  - Trace each incoming WS message (kind, len) to prove whether `PeerJoined` is delivered.

- Compatibility probe
  - If POST/GET to `/sessions/:id/webrtc/*` returns 404, retry once against `/fastpath/sessions/:id/webrtc/*` (and vice‑versa). Warns with both URLs. This is temporary to prove endpoint drift.

## Workarounds (short term)

- For server endpoint drift: keep the compatibility retry until the service advertises the correct signaling URL consistently.
- For SSH teardown: pass `--ssh-keep-host-running` during bootstrap when tailing logs or diagnosing hangs.

## Proposed “right” long‑term fixes

1) Signaling contract
- Always include `role:"offerer"` on the host’s WebRTC transport metadata; `role:"answerer"` for participants.
- Standardize on one REST path (prefer `/fastpath/sessions/:id/webrtc`), and update all clients to use the advertised path. Remove compatibility retry after rollout.
- Ensure `PeerJoined(Client)` is broadcast to Offerer reliably. Add server tests:
  - Host registers → WS: JoinSuccess(peers[]) includes server role only.
  - Participant joins → Offerer receives `PeerJoined(Client)`.
  - Offer POST 200; client fetch 200 → DC open.

2) SSH bootstrap semantics
- Don’t kill the remote host on transport connect. Instead, gate teardown on “sync handshake complete”. Define completion as:
  - Host sent Hello + grid descriptor; client acked first backfill (or a single frame delivered).
- Keep a `--detach-remote` (rename of `--ssh-keep-host-running`) to explicitly leave the host running independently. Make `--stream-ssh` purely about log tailing.
- If sync does not complete within N seconds (e.g., 20s), surface a clear failure (“Remote host failed to sync; leaving SSH open”) rather than silently killing the host.

3) Client/host hardening
- Treat session role as authoritative when metadata is missing/contradictory: Host→Offerer, Participant→Answerer.
- Bound the “wait for offer” loop with a diagnosed failure (not a spinner that runs forever), suggesting next steps (check signaling URL/PeerJoined delivery).

## Acceptance criteria

- Happy path: `beach ssh …` prints “Connected – syncing…”, then proceeds to a live terminal within a few seconds (WebRTC preferred; WebSocket fallback acceptable), with host continuing to run unless explicitly detached.
- Logs show: `peer_joined` → `offer created` → `post_sdp 200` → client `fetch_sdp 200` → `offer applied` → DC open → Hello/backfill processed.
- No infinite 404 polling, no UI spinner hang, no premature host teardown.

## Action items

- [Server] Standardize advertised signaling URL (choose `/fastpath/sessions/:id/webrtc`), add tests, deploy.
- [Server] Ensure `PeerJoined(Client)` is delivered to Offerer on WS join.
- [CLI] Change ssh bootstrap to wait for “sync complete” notify before tearing down SSH; introduce `--detach-remote`.
- [Client/Host] Keep the new tracing; after server rollout, remove compatibility retry and simplify logging.

## Appendix: Log snippets

- Client indicating path drift:

```
fetch_sdp.offer … url=https://api.beach.sh/sessions/<id>/webrtc/offer … result=Ok(404)
```

- Offerer missing PeerJoined (no `peer_joined`, no `offer posted`).

- UI stuck after WebSocket connect, caused by host teardown:

```
transport established transport="websocket" url=wss://api.beach.sh/ws/<id>
… UI: “Connected – syncing remote session…” (never progresses)
```


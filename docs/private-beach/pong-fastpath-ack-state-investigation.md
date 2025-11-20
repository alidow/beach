# Pong Fast-Path Ack/State Investigation – 2025‑11‑20

## Summary

We can no longer get the pong fast-path smoke test to reach “fast-path ready for lhs/rhs”. The controller (`mgr-actions`) channel handshakes successfully, but the ack (`mgr-acks`) and state (`mgr-state`) data channels never report readiness: the host times out waiting for the `__ready__` sentinel even though the manager sends it repeatedly. This blocks the agent from switching to fast-path (so we still see duplicate balls / HTTP fallback).

This doc captures the current evidence, everything we tried, and open questions for a second pair of eyes.

## Environment and Setup

- Script under test: `scripts/pong-fastpath-smoke.sh` (auto restarts docker compose, creates a temporary private beach, runs `pong-stack.sh` with `--setup-beach`, collects artifacts).
- Host env:
  - `direnv` populates `BEACH_ICE_PUBLIC_IP=192.168.1.245` (my LAN IP) and `BEACH_ICE_PUBLIC_HOST=192.168.1.245`.
  - `STACK_ENV_ICE_IP/HOST` inherit those values; the script fails fast if they are unset.
  - Docker compose publishes UDP 62000‑62100 and resolves `host.docker.internal`.
- Current instrumentation:
  - `apps/beach-manager/src/fastpath.rs` now calls `install_ready_sentinel` for each data channel (actions/acks/state). We send the `__ready__` sentinel on `dc.on_open` and then re-send every 2 s up to eight times, logging success/failure.
  - `apps/beach/src/server/terminal/host.rs` logs whenever it sees `__ready__`/`__offer_ready__` on the controller channel (`controller.fast_path_state`) and the general input listener (`sync::incoming`).

## Evidence

Latest repro: `temp/pong-fastpath-smoke/20251120-065940/…`

1. **Controller channel succeeds**
   - `beach-host-lhs.log:1544` – `received __ready__ sentinel from answerer peer_id=585f5375-9037…`
   - `beach-host-lhs.log:1595` – `fast_path controller channel active`
   - `beach-host-lhs.log:1597` – `fast path controller channel ready` (mgr-actions)
   - HTTP poller pauses (`beach-host-lhs.log:1703`).

2. **Ack/state channels fail handshake**
   - `beach-host-lhs.log:1334` – `beginning to poll for __ready__ sentinel peer_id=08bbcc89…` (mgr-acks channel).
   - `beach-host-lhs.log:2296` – `readiness handshake polling finished … ready_seen=false`.
   - `beach-host-lhs.log:2297` – `closing peer connection: did not receive __ready__ sentinel`.
   - Same sequence for peer `6b6d5e84…` (mgr-state) at `1427` / `2387`.
   - No log entries from our new instrumentation (no `controller.fast_path_state` or `sync::incoming` “received fast-path sentinel” lines for these channels).

3. **Manager confirms it sends the sentinel repeatedly**
   - `docker compose logs beach-manager | rg 'sent fast-path ready sentinel' | rg 24d0738e-…` shows attempts 0‑6 for all three channels of LHS. Example lines:
     ```
     2025-11-20T11:59:53.909Z sent fast-path ready sentinel session_id=24d0738e… channel=mgr-actions attempt=0
     2025-11-20T12:00:02.237Z sent fast-path ready sentinel session_id=24d0738e… channel=mgr-state attempt=4
     ```
   - No `failed to send` warnings printed.

4. **State listener times out**
   - `beach-host-lhs.log:2518` – `timed out waiting for fast-path state channel … timeout_secs=10`.

5. **Agent still gated on transport**
   - `agent.log` repeats `DEBUG readiness blocked for lhs: transport`.

## Things We Tried

1. **ICE config sanity**
   - Verified `.envrc` exports `BEACH_ICE_PUBLIC_IP=192.168.1.245`.
   - `pong-fastpath-smoke.sh` consumes that via `STACK_ENV_ICE_IP/STACK_ENV_ICE_HOST` and passes the values to `docker compose build/up`.
   - Manager logs confirm it’s configured with the LAN IP: `configured fast-path ICE hints … public_ip_hint="192.168.1.245"`.
   - Host logs show srflx/private candidates for that IP, so connectivity should be possible.

2. **Manager fast-path instrumentation**
   - Added `install_ready_sentinel` to `apps/beach-manager/src/fastpath.rs`.
   - Repeated sentinel transmissions confirmed via logs; also continue sending `__ready__` every 2 s (up to 8 times) in case the first packet gets lost.

3. **Host-side logging**
   - Logged any inbound `__ready__`/`__offer_ready__` on the controller path (`controller.fast_path_state` and `sync::incoming`). Those logs appear for the actions channel but never for the ack/state peers.

4. **Ensure CLI in container uses new bits**
   - Rebuilt `beach` inside the `beach-manager` container before each run (`cargo build --bin beach` from `/app`), so the headless players/agent have the latest fast-path client code / instrumentation. Verified `PONG_DEBUG_MARKER: mgr-state build active` appears at startup.

5. **Multiple reruns / artifact collection**
   - Reproduced across several runs (`20251119-201308`, `20251120-065940`, etc.). Artifacts live under `temp/pong-fastpath-smoke/<timestamp>/…`.

## Hypotheses / Open Questions

1. **Are ack/state channels publish-only on the host?**
   - The CLI creates all channels as ordered, reliable data channels (see `crates/beach-buggy/src/fast_path.rs:804`). There’s no `negotiated` flag, so they should be bidirectional. Still, the host never logs any inbound message unless we use the controller channel—maybe the ack/state transports drop answerer-originated text frames?

2. **Handshake ordering mismatch?**
   - Manager sends `__ready__` immediately on `dc.on_open`. The host’s `WebRtcTransport` might not yet have registered the `wait_for` channel for ack/state, so the first sentinel could land before the transport is installed and subsequent sends could hit after the host already closed the channel. Need to verify whether `channels.publish` fires before `state_channel_listener` waits.

3. **SCTP/network issue specifically affecting ack/state**
   - Maybe those channels are sharing the unordered/unreliable settings and the `send_text` call is failing silently at the SCTP layer? We have no logs confirming manager’s `send_text` failure—should we instrument the `pion` data-channel to capture errors or inspect SCTP counters?

4. **Controller-only span context?**
   - `maybe_spawn_state_channel_listener` only runs if the last channel label is `mgr-actions`. Are we missing a sequencing step that keeps ack/state waiting on `mgr-actions` being ready first? (From the logs, actions do attach, so this path should run.)

5. **Host intentionally ignores `__ready__` on non-controller channels?**
   - Search shows only the controller/input (`sync::incoming`) loops strip `__ready__`. `fast_path_state_channel.activate` never logs anything, which implies the WebRTC channel never arrived in `WebRtcChannels`. Need to verify `channels.publish` is called for ack/state (should happen inside `FastPathClient::create_channel`).

## Next Steps

1. **Capture SCTP/data-channel diagnostics**
   - Either run with `RUST_LOG=webrtc::data_channel=trace` or instrument `RTCDataChannel::send_text` / `on_message` via a custom wrapper to confirm whether the `__ready__` packets are created / delivered.
2. **Inspect `WebRtcChannels` lifecycle**
   - Confirm that the ack/state `Arc<dyn Transport>` objects are published into `WebRtcChannels` on the host (e.g., temporary logging inside `WebRtcChannels::publish` or `FastPathChannels::create_channel`).
3. **Consider reproducing outside pong harness**
   - A minimal `cargo run --bin beach … host` + `beach-manager` handshake might tell us if this repro is specific to the CLI headless players or a general host issue.
4. **Double-check ack/state channel creation parameters**
   - Manager creates them via `FastPathSession::set_remote_offer`, mapping `mgr-acks`/`mgr-state` to data channels. Need to ensure we didn’t accidentally mark them as datachannels the offerer never listens to (e.g., missing `await_for_channel` for ack).

If anyone picks this up, please read through the referenced artifacts/log lines above and help brainstorm why the host CLI never sees the ready sentinel on `mgr-acks`/`mgr-state` despite the manager sending them.

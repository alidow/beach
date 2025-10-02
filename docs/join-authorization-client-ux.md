# Join Authorization — Client Waiting UX Plan

This document complements `docs/join-authorization-impl-plan.md` with the client‑side experience while a join request is pending host approval. The goal is to make the waiting period feel intentional and responsive, with clear progress signals and graceful exits.

## Problem

With host‑side authorization enabled, a client can complete transport setup (WebRTC/WebSocket) but not yet receive terminal sync frames because the host hasn’t approved the join. Today the client TUI shows an empty grid with no feedback. Users can’t tell if they’re connected, waiting, or stuck.

## Goals
- Show a clear, animated “waiting for host approval” state once connected but before handshake frames (Hello/Grid) arrive.
- Avoid sending any keystrokes to the host before authorization completes.
- Provide a reassuring sense of progress and time passage (spinner/animation, occasional status updates).
- Communicate outcomes (approved, denied, timeout, disconnected) in friendly language.
- Keep behavior backward‑compatible with servers that don’t send explicit status signals.

## Implementation Status
- ✅ Rust CLI client: status line overlay with ASCII spinner (`-|/`), 750 ms fallback, `beach:status:*` handling, and pre-handshake input gating (`apps/beach-human/src/client/terminal.rs`).
- ✅ Host runtime emits `beach:status:approval_{pending,granted,denied}` hints so the client can surface progress immediately.
- ⬜ Optional CLI join-command pre-TUI spinner (nice-to-have).
- ✅ Viewer can supply an identifier via `--label` (CLI) or `?label=` (web) that the host now displays during authorization.

## High‑Level UX

States shown to the user:
- Connecting… (optional: during transport negotiation in `beach join` before the client TUI starts)
- Connected — Waiting for host approval… [spinner]
- Approved — Syncing… (brief)
- Denied or Disconnected — Show reason and exit

Presentation:
- Use the existing status line in the TUI’s bottom bar for messaging and animation; reserve body area for the grid once the handshake completes.
- Spinner animation uses ASCII frames (`-|/`) refreshed every ~120 ms by a local timer (heartbeats simply keep the loop active).
- After 10 s/30 s with no approval (and no host-supplied copy), swap the text for gentle hints (“Still waiting... hang tight.” / “Still waiting... ask the host to approve.”).
- On approval (either `beach:status:approval_granted` or `Hello`), briefly show “Approved - syncing...” for about 1.2 s before clearing the message.
- On denial or early channel close before Hello, show “Join request was declined by host” or “Disconnected before approval” and exit after a short pause so the message is visible.

## Signals & Triggers

Client triggers:
- Data channel open with no `Hello` → enter “waiting for approval” after a 750 ms grace period if the host hasn’t already sent `approval_pending`.
- Local timer (120 ms cadence) → advance the spinner; heartbeats simply keep the main loop responsive.
- First `HostFrame::Hello` or `approval_granted` signal → transition to “approved/syncing” then normal rendering.

Optional server signals (now implemented by the host runtime):
- Immediately after the data channel is up, send `beach:status:approval_pending` so clients can display feedback without guessing.
- On denial, send `beach:status:approval_denied` before closing the channel.
- On approval, optionally send `beach:status:approval_granted`; the client also upgrades when `Hello` arrives.

These text messages are only consumed pre‑hello; older CLI builds ignore them safely.

## Client Implementation Plan (TerminalClient)

1) Pre‑handshake status and animation
- Add `pre_handshake: bool` or reuse “hello not yet received” to drive the waiting UI.
- Add a lightweight spinner state (index + last_tick). On each render pass or heartbeat, update the spinner string and set `renderer.set_status_message(Some("Connected — waiting for host approval … <spinner>"))`.
- Clear status message after handshake completes.

2) Input gating before Hello
- Ensure no input is forwarded to the host before `HostFrame::Hello` is processed. In `pump_input`/`send_input`, ignore or buffer keys while `pre_handshake` is true. On handshake, drop the buffer to avoid unintended commands.

3) Heartbeat‑driven ticks
- On `WireHostFrame::Heartbeat`, if `pre_handshake`, advance spinner and mark dirty. This makes the UI feel “alive” even if no local input occurs.
- Also advance via a local interval to be independent of heartbeat frequency.

4) Text control messages (optional, pre‑hello only)
- In the `Payload::Text` branch, recognize `beach:status:*` while the handshake is incomplete:
  - `approval_pending` → enter/refresh the waiting state (custom message respected).
  - `approval_denied` → show denial messaging, schedule a graceful exit, and ignore subsequent input.
  - `approval_granted` → transient “Approved - syncing...” message even before `Hello` lands.
- Ignore other text payloads; all text is ignored post‑hello.

5) Friendly disconnects
- If the transport closes before Hello, display a final message (“Disconnected before approval”) and exit cleanly after a short pause.

6) Join command pre‑TUI spinner (optional nice‑to‑have)
- In `handle_join`, before the transport is established, print a simple CLI spinner (“Connecting to session…”) until the transport is ready. Once ready, start the TUI.

## Host Integration

- Keep heartbeats active during authorization so clients get liveness ticks.
- Optionally send the `beach:status:approval_pending` control message immediately after the data channel is up.
- On denial, send `beach:status:approval_denied` and close the transport.
- Don’t send `HostFrame::Hello`/Grid until approval.

## Edge Cases
- Non‑TTY clients: render a simple one‑line textual spinner without alternate screen, or skip animation and just print progress lines.
- Fast approvals: the waiting UI may flash; throttle to show at least one spinner frame or suppress if Hello arrives within ~150ms.
- Multiple reconnects: reset to waiting state on reconnect; reuse the same UI logic.

## Testing
- Unit: spinner tick state machine; gating of `send_input` prior to Hello; text control parsing.
- Integration: use IPC/WebRTC test pairs to simulate `approval_pending` → Hello, denial then close, and heartbeat‑only waiting.
- Manual: verify status line animations at different terminal sizes; ensure minimal CPU usage.

## Accessibility
- Use high‑contrast status messages. Avoid relying solely on color.
- Keep the spinner ASCII‑safe; avoid Unicode animations when `LC_ALL=C` or simple mode is selected.

## Rollout
- Clients can ship UX first (waiting overlay + input gating) without any server changes.
- Add server control messages once the host feature lands for richer feedback.

By coupling a minimal pre‑handshake overlay with heartbeat‑driven animation and strict input gating, the client experience remains reassuring and safe while the host decides whether to admit the viewer.

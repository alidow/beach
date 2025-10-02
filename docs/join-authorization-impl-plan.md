# Client Join Authorization Prompt — Implementation Plan

This document proposes and details a host-side authorization prompt for new client connections to the `beach-human` server runtime. By default, new clients require explicit approval from the host via a full-screen prompt in the host terminal. The feature can be disabled via a CLI flag (`--allow-all-clients`).

## Background & Current State

`beach-human` hosts a PTY and shares live terminal state with one or more remote viewers over transports (WebRTC, WebSocket, IPC). The host runtime attaches transports and starts streaming after transport negotiation completes.

Relevant datapaths (server/host side):
- Initial WebRTC negotiation acceptor: `apps/beach-human/src/main.rs:2865` (`spawn_webrtc_acceptor`).
- Subsequent viewer accept loop (multi-peer): `apps/beach-human/src/main.rs:2997` (`spawn_viewer_accept_loop`).
- Transport registration and sync start happens when we push the shared transport and send `ForwarderCommand::AddTransport` (handshake frames are emitted in `initialize_transport_snapshot`, invoked by the update forwarder after registration).
- Local preview transport (IPC) is established when `--local-preview` is used; it is not a remote viewer and should not require approval.
- Interactive detection and raw-mode guard are already present (`apps/beach-human/src/main.rs:340`, `apps/beach-human/src/main.rs:2177`).
- Local stdin is forwarded to the PTY via `spawn_local_stdin_forwarder` (`apps/beach-human/src/main.rs:2489`).

Observations:
- If we gate the “attach transport” step and only proceed after authorization, we prevent sending `HostFrame::Hello`, snapshots, and deltas to unauthorized peers. The update forwarder only begins sending after a transport is registered via the forwarder channel.
- To avoid accidental consent while prompting, we must pause local stdin → PTY forwarding and use a deliberate confirmation gesture.
- In bootstrap/non-interactive modes, full-screen TUIs are not viable; policy should default to allow-all or require an explicit opt-in for prompting.

## Problem Statement

Current behavior accepts viewers as soon as negotiation completes. This can surprise hosts or leak terminal state if a share code is reused unintentionally. We want a simple, safe approval step that’s on by default but unobtrusive for power users (opt-out flag).

## Goals
- Default-deny new remote clients until the host explicitly authorizes them in the host terminal.
- Provide `--allow-all-clients` to disable prompting for frictionless workflows and automation.
- Ensure no keystrokes typed by the host before or during prompt display count as approval.
- Work in single-host, multi-viewer scenarios (queue if multiple joins arrive).
- Avoid sending any server frames (hello/snapshots/deltas) prior to approval.
- Clear observability (logs, trace) for prompts, decisions, timeouts, and drops.

## Implementation Status
- ✅ Host runtime: CLI flag, stdin gate, interactive prompt, and join gating added in `apps/beach-human/src/main.rs` (emits `beach:status:*` hints for waiting clients).
- ✅ Rust CLI client: status-line waiting UX, ASCII spinner, timed hints, and pre-handshake input gating in `apps/beach-human/src/client/terminal.rs`.
- ✅ beach-web: overlay UX with CSS spinner, timed hints, and status handling in `apps/beach-web/src/components/BeachTerminal.tsx`.
- ✅ Host prompt now surfaces viewer metadata (optional label + remote address) collected from the signaling layer, with CLI/web clients able to opt-in via `--label` or `?label=`.

## Non-Goals
- We do not attempt to implement cryptographic identity or long-lived trust lists in this iteration.
- We do not add client-side UI for authorization.
- No persistence of authorization across sessions (unless added as a follow-up).

## UX & CLI

Flag: `--allow-all-clients`
- Type: boolean
- Scope: `HostArgs`
- Default: false (prompt enabled)
- Behavior: when true, all clients are auto-accepted (existing behavior), bypassing the prompt.

Prompt behavior (interactive TTY only):
- On a new client request, switch to a full-screen prompt in the alternate screen buffer.
- Show: peer id (if available), handshake id, transport kind, and instructions.
- Accept input only after a short guard interval; require a deliberate action (e.g., type `yes` + Enter or a specific key binding like F2) to authorize.
- Deny with ESC/q/N/timeout. Denied connections are dropped without starting sync.
- Restore prior terminal state when the prompt completes.

Policy matrix:
- Interactive TTY, not bootstrap: prompt (default deny until explicit approval).
- Bootstrap JSON mode (non-TTY): bypass prompt; allow all (and log policy). Future: add `--deny-on-noninteractive` if needed.
- Local IPC preview: bypass prompt.

## Architecture & Components

New components:
- `Authorizer`: orchestrates authorization decisions.
  - Modes: `AllowAll`, `Interactive`, `Disabled` (alias of `AllowAll` for bootstrap/non-interactive).
  - API: `async fn authorize(&self, meta: JoinMeta) -> JoinDecision`.
- `JoinMeta`: peer metadata available during negotiation (peer_id, handshake_id, `TransportKind`).
- `JoinDecision`: `Allow`, `Deny`, `Timeout`.
- `HostInputGate`: gate for local stdin → PTY forwarder.
  - `pause()` to stop forwarding; optionally buffer bytes while paused.
  - `resume(flush: bool)` to resume and optionally flush buffered bytes (excluding decision keys).
- `TuiPrompt`: renders and handles the full-screen authorization prompt using `crossterm`.
  - Enters alternate screen and exits cleanly, coordinates with raw-mode guard.

Data flow:
1. Transport negotiation completes (initial or viewer join).
2. Before registering transport and starting I/O, call `authorizer.authorize(meta).await`.
3. If `Allow`, register transport (push to shared list, send `ForwarderCommand::AddTransport`, spawn input listener).
4. If `Deny`/`Timeout`, drop the transport quietly; optionally send a best-effort text notice and close.

## Detailed Implementation Plan

1) CLI additions
- Modify `HostArgs` (apps/beach-human/src/main.rs:153) to include `#[arg(long = "allow-all-clients", action = clap::ArgAction::SetTrue, help = "Disable interactive approval; automatically accept all clients")] pub allow_all_clients: bool`.
- In `handle_host`, compute `interactive` as it is today (`apps/beach-human/src/main.rs:340`). Determine authorizer mode:
  - If `args.allow_all_clients` or not `interactive` or in bootstrap JSON mode → `AllowAll`.
  - Else → `Interactive`.

2) HostInputGate
- Add a small module near existing stdin utilities (`apps/beach-human/src/main.rs`):
  - `struct HostInputGate { paused: AtomicBool, buffer: Mutex<Vec<u8>> }`.
  - `fn pause(&self)` sets paused; `fn resume(&self, flush: bool)` flips paused and optionally flushes to the PTY writer.
- Update `spawn_local_stdin_forwarder` (`apps/beach-human/src/main.rs:2489`) to:
  - Check `gate.paused()` before writing; if paused, append to buffer (up to a bounded size) and continue.
  - On resume with `flush=false`, discard buffered bytes to avoid accidental command execution.
- Gate is owned by `handle_host` and passed to `spawn_local_stdin_forwarder`.

3) Authorizer
- Define `JoinMeta { peer_id: Option<String>, handshake_id: Option<String>, kind: TransportKind }`.
- Define `JoinDecision { allowed: bool }` or an enum.
- Implement `Authorizer::authorize`:
  - `AllowAll` returns `Allow` immediately.
  - `Interactive`:
    - Pause `HostInputGate`, sleep ~50ms guard to avoid capturing in-flight typing.
    - Run `TuiPrompt::run(meta)` on a blocking thread (`tokio::task::spawn_blocking`) to avoid blocking the runtime.
    - On decision, resume input gate (no flush by default), return `Allow` or `Deny`.
    - Timeout (e.g., 60s) yields `Deny`.

4) TuiPrompt
- Implement with `crossterm`:
  - Save current state; temporarily disable raw mode (use `RawModeGuard` logic) and enter alternate screen.
  - Render a simple approval screen with clear instructions (accept = `yes` + Enter or `F2`; deny = ESC/q, or wait for timeout).
  - Ensure cleanup: leave alternate screen, restore cursor, re-enable raw mode if previously enabled.
  - Validate behavior when stdout/stderr aren’t ttys (no-op and return `Allow` if authorizer mode is `AllowAll`).

5) Wire into accept loops
- `spawn_webrtc_acceptor` (apps/beach-human/src/main.rs:2865):
  - After `transport` is ready but before `SharedTransport::new(...)` is added and before `ForwarderCommand::AddTransport`, call `authorizer.authorize(meta).await`.
  - On `Allow`: proceed as today. On `Deny`/`Timeout`: do not push to `transports`, do not spawn input listener; drop transport.
- `spawn_viewer_accept_loop` (apps/beach-human/src/main.rs:2997): same gating before registering the viewer transport.
- Exempt local IPC preview transport from gating.

6) Logging & Telemetry
- Add `tracing` events at INFO/DEBUG:
  - Prompt opened/closed, decision, and timeout with `peer_id`/`handshake_id` when available.
  - Allow/deny outcomes, and policy chosen (interactive/allow-all) once at host startup.

7) Edge Cases & Policy
- Non-interactive or bootstrap JSON mode: force `AllowAll`, log policy selection.
- Multiple concurrent join attempts: serialize prompts (queue requests). A simple `tokio::Mutex` in `Authorizer` can ensure only one prompt is shown at a time; later requests wait their turn.
- Transport lifetime: ensure dropping unregistered transports frees associated tasks and channels (aligns with current behavior—no forwarder sink is created, and input listener isn’t spawned).

8) Testing Strategy
- Unit tests:
  - `HostInputGate` pause/resume logic; buffering and discard/flush semantics.
  - `Authorizer` decision paths in `AllowAll` vs. fake `Interactive` (inject a test decision provider).
- Integration tests:
  - Simulate a negotiated transport (use `TransportPair` IPC or WebRTC test pair) and verify that when authorization is denied, no hello/snapshot frames are sent (sinks remain empty).
  - Verify that when authorization is allowed, `HostFrame::Hello` and subsequent frames are sent and client input is processed.
  - Non-interactive mode: confirm prompts are bypassed and transports attach automatically.
- Manual sanity:
  - Run host with prompt enabled; attempt to join; verify TUI, accept/deny flow, and that local typing during the switch doesn’t leak into consent.

9) Rollout Plan
- Land behind the new default (prompt on). Provide `--allow-all-clients` for opt-out.
- Add an env var override for CI: `BEACH_ALLOW_ALL_CLIENTS=1` to simplify automated flows.
- Document behavior in the host banner and `--help`.

10) Risks & Mitigations
- Risk: Accidental consent due to typing during mode switch → mitigate with input gate pause + short guard sleep and deliberate confirmation sequence.
- Risk: UI deadlocks or leaves terminal in bad state → ensure prompt cleanup is exception-safe and always restores raw mode and screen state.
- Risk: Delayed acceptance may cause signaling retries → current supervisor loops handle reconnects; we are not registering transports until approved, so no server-state leak.

## References
- Interactive detection and raw mode guard: `apps/beach-human/src/main.rs:340`, `apps/beach-human/src/main.rs:2177`.
- Local stdin forwarder: `apps/beach-human/src/main.rs:2489`.
- Initial negotiation acceptor: `apps/beach-human/src/main.rs:2865`.
- Multi-viewer accept loop: `apps/beach-human/src/main.rs:2997`.
- Server handshake emission happens only after transport registration in the forwarder (see `initialize_transport_snapshot` call sites and `ForwarderCommand::AddTransport`).

## Future Enhancements
- “Always trust this peer for the lifetime of this session” selection in the prompt.
- A host policy file or allowlist by peer id.
- A non-fullscreen minimal prompt option and/or sound/visual notification with deferred queue.
- Structured denial notice sent to the client over a control channel.

This plan constrains the change to the server process, provides a safe default, and minimizes the risk of accidental authorization while keeping the existing fast-path available through `--allow-all-clients`.

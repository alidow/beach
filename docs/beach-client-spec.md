# Beach Client Spec (Rust TUI + Portable Design)

Status: Draft v1

Owner: beach

Scope: Native Rust client TUI, aligned with a future browser client (TypeScript) using a shared protocol and test corpus. Targets Windows, macOS, Linux terminals. Interoperates with `apps/beach` server and `apps/beach-road` signaling.

---

## Vision & UX (Start With The User)

The user runs a Beach server with a command (or none) and shares a session URL. From any machine:

```
beach join <session-id-or-url>
```

The client starts a full‑screen TUI that feels like the server’s terminal:
- Colors and style match closely; no surprise wrapping; no flicker.
- Scrolling up is instant, even on slow links, thanks to buffered history.
- Typing is responsive: local predictive echo shows the user’s keystrokes immediately (underlined). The display self‑corrects seamlessly when the server confirms or when other input wins the race.
- If the connection drops, the display stays usable; it resumes automatically with a fresh snapshot.

Delightful defaults:
- If the session is public and no passphrase was provided, the client prompts for it in a friendly interstitial (or reads from env/credentials if present).
- The TUI guides the user with minimal, discoverable controls (help overlay, scroll keys).
- The experience “just works” on lossy, high‑latency links (cellular, coffee‑shop Wi‑Fi).

---

## Modes & First‑Run Experience

Public Beach
- Join: `beach join <url|id> [--passphrase <code>]`. If no code provided, prompt.
- URL clarity: `public.<host>/...` visibly indicates public mode.
- Security: session description is sealed with the code (not sent in plaintext); a short post‑connect handshake binds to the exact connection (see transport specs). Minimal client may defer the handshake, but must not leak codes.

Private Beach
- Requires `beach login` (Clerk). Join uses the selected profile; URL is `private.<host>/...`.
- The passphrase can be an optional extra gate.

Interstitial for passphrase (public):
- Full‑screen prompt with emphasis on the short code; clear indicator for input or `--passphrase` usage; timeout on inactivity; never prints secrets to stdout/stderr logs.

---

## Terminal Rendering Model

Authoritative state on the server:
- The server tracks a terminal grid and history (already implemented: Grid, GridView, GridHistory).
- The server can derive snapshots and deltas.

Client rendering:
- The client maintains a local viewport state (width, height, top line index) plus a scrollback ring buffer sourced from the server.
- The client applies deltas optimistically as they arrive on an unreliable channel; gaps trigger resyncs.
- Colors and attributes are rendered faithfully. Font face is terminal‑local; the client honors ANSI style and color tables. In the future, the server can expose palette metadata to help match theme.

Dimension rules (critical UX):
- The server’s width is authoritative. The client grid width MUST match the server; no soft wrapping.
- If the user’s terminal is narrower than the server width, the client provides horizontal scroll (no auto‑wrap) and explains this state in a subtle status line.
- Height can vary; the client requests more rows than visible (overscan) and keeps a buffer for instant scrollback. Resize events are reported upstream; the server can adjust the viewport derivation.

Scrolling:
- Vertical: Up/Down/Page/Home/End adjust the client’s `top_line`. If the user nears the buffer head, the client updates the subscription to pull more history (earlier `from_line`).
- Horizontal: Left/Right to pan when the local terminal is narrower than the server width; show a column indicator.

---

## Input & Predictive Echo (Mosh‑Inspired)

Goals:
- Zero‑latency local feedback when typing, even on high RTT.
- 100% ordered input at the server PTY, across multiple clients and the server’s own stdin.
- Seamless correction if other input lands first.

Mechanics:
- Local predictive echo: When the user types, the client renders the character locally, underlined (predictive). The client sends an Input message with a `client_seq` and `client_id` on the reliable control channel.
- Server serialization: The server assigns a global `apply_seq` order to each input as it enqueues to the PTY.
- Acknowledgments: The server returns InputAck with the mapping `{client_seq -> apply_seq}` and the current render `version`. On ack, the client removes underline for that input. If the authoritative output shows a different result (due to other inputs or program state), the next delta/snapshot corrects the display.
- Conflicts/races: If an older predictive character is invalidated by intervening inputs, the authoritative delta supersedes the prediction. The user rarely notices; underline disappears only on confirmed characters.

Edge cases:
- Pasted input: Batch predictive echo (underline) with throttled acks.
- Binary/escape heavy apps (vim, tmux): Predictive echo is still local text; authoritative output always wins. The width constraint avoids wrap mismatches.

---

## Transport & Channels (Client Perspective)

- Control channel (`beach/ctrl/1`): reliable, ordered. Carries: handshake, auth, input, acks, resync requests, resizing, and small beacons.
- Output channel (`beach/term/1`): unreliable, unordered. Carries: terminal deltas/snapshots (drop‑old, send‑latest).
- Startup order: open control; complete security handshake (when enabled); then open output. Minimal client may receive output over control as a fallback before the second channel exists.

Resilience:
- If output channel stalls/fails, fall back to control channel.
- On disconnect, keep the last snapshot on screen, queue predictions locally, and attempt fast reconnect. On resume, request a fresh snapshot and replay final acks.

---

## Subscriptions, Overscan, and Instant Scrollback

- Subscription fields: `{ from_line, height }` relative to server history.
- Overscan policy: request `height = visible_rows * k` (e.g., k=2) and adjust `from_line` so the buffer covers user’s likely scroll.
- Scroll actions: when the user scrolls near the buffer boundary, send a new subscription request moving `from_line` further back. Keep a small hysteresis to avoid thrash.
- The server responds by streaming deltas/snapshots that align with the new subscription window.

---

## Message Shapes (Portable)

Control (reliable):
- Input { client_id: String, client_seq: u64, bytes: Vec<u8> }
- InputAck { client_seq: u64, apply_seq: u64, version: u64 }
- Ack { version: u64 }  // last applied render version
- ResyncRequest { reason: String }
- Viewport { cols: u16, rows: u16 }  // client view (for UI), server width remains authoritative
- Subscribe { from_line: u64, height: u16 }  // overscan window
- Heartbeat { t: i64 }, HeartbeatAck { t: i64 }

Output (unreliable preferred):
- Delta { base_version: u64, next_version: u64, ops: GridDelta }
- Snapshot { version: u64, grid: Grid, compressed: bool }
- Hash { version: u64, h: [u8; 32] } // optional integrity beacon

Notes:
- These shapes must be identical for Rust and TypeScript clients. Prefer a JSON schema or a protobuf with deterministic encoding for cross‑language tests.

---

## Minimal Client (Phase 2a) – Exact Scope

Must‑have:
- CLI: `beach join <url|id> [--passphrase]` with prompt interstitial if missing.
- Transport: open control channel; receive output over control (fallback); output channel optional in 2a.
- TUI: full‑screen, render server grid; local predictive echo (underlined) for typed ASCII; hide underline on ack (stub ok if server acks not yet wired).
- Dimensions: enforce server width; show horizontal scroll indicators when local terminal narrower; vertical scrolling with overscan subscription (request visible_rows*2).
- Resilience: if connection drops, keep last screen; auto reconnect; on resume, request snapshot.
- Logging: no noisy stderr; respect `--debug-log`.

Out‑of‑scope for 2a (will come in later phases):
- Sealed signaling and handshake (integrated later; do not leak secrets meanwhile).
- Full Clerk flow (private mode).
- Rich theme/palette sync; advanced compression; PAKE.

---

## Edge Cases & Behavior

- Lossy links: output deltas may drop; client requests resync via control when gaps detected.
- High latency: predictive echo masks RTT; acks remove underline; conflicting output corrects predictions.
- Connection drops: retain display; rejoin; fresh snapshot; replay final acks.
- Multiple writers: server serializes inputs; client predictions are cosmetic and converge to authoritative state.
- Narrow terminals: enforce server width; avoid wrap; provide horizontal panning.

---

## Testing Strategy (Shared Across Rust and TS)

Unit tests (Rust): `apps/beach/src/tests/client/`
- `join_flow.rs`: mock transport; ensures join → control channel open → initial snapshot render.
- `predictive_input.rs`: simulate input, assert underline on prediction, underline removal on ack, correction on conflict.
- `scroll_prefetch.rs`: verify overscan subscription requests and instant scrollback.
- `reconnect_resync.rs`: simulate disconnect; ensure resume with new snapshot.
- `order_guarantees.rs`: interleave two clients’ inputs; ensure server serialization reflected in acks.

Cross‑language golden tests: `tests/shared/`
- JSON fixtures for Grid → Delta → Snapshot sequences with expected viewport outcomes.
- Deterministic replayer verifies both Rust and TS renderers produce identical frames.

Manual tests:
- High‑loss simulator (tc/netem) to validate delta loss and resync.
- Latency injection to evaluate predictive echo UX.

---

## Portability Notes (Toward Browser Client)

- Keep message shapes identical and wire types platform‑neutral.
- Minimize platform‑specific TUI logic; isolate rendering core so it can be reused in TS with xterm.js.
- Avoid depending on terminal quirks; prefer an internal grid model updated by deltas, then painted.

---

## Open Questions

- Predictive echo policy for non‑printable/control sequences (vim): when to suppress vs. show placeholders.
- Horizontal panning UX defaults (step size, indicators).
- Snapshot compression (zstd vs. none) and thresholds.
- Theme/palette sync protocol.


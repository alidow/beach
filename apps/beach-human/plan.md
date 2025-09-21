# Session + Runtime Implementation Plan

This document tracks the remaining milestones for the new `beach-human` stack. Each phase is scoped so we can land incremental tests and manually exercise the CLI as soon as possible.

## ✅ 1. Session Wiring Pass

- Host emits heartbeat/sync primitives over the negotiated transport.
- Join flow subscribes and logs events for smoke testing.
- Unit tests validate the mock transport loop.

## ✅ 2. Server Runtime (Milestone A)

- PTY wrapper, emulator, and cache producer running inside `server::terminal`.
- Terminal runtime pumps diffs into the shared grid and `TerminalSync`.
- Sync publisher sends structured snapshots/deltas to clients.

## 🚧 3. Client Runtime (Milestone B)

- ✅ Minimal client consumes sync frames, renders a text viewport, and returns keystrokes/paste data to the host.
- ✅ Swap in the ratatui-based renderer (copied from the legacy `apps/beach` client) with scrollback, selection, and cursor/status overlays.
- ✅ Reintroduce predictive echo / resize propagation so local typing feels immediate while the PTY catches up.
- ✅ Negotiate a WebRTC data channel via beach-road signaling so host ↔ client traffic stays off the websocket path.
- ✅ Host CLI mirrors PTY output locally while continuing to stream deltas to remote clients.
- ⏳ Add `--debug-matrix` / transcript introspection flags and document workflows for debugging.

- ✅ Optional `--local-preview` flag to attach a first-party terminal client without disturbing the host shell baseline.

## 🔜 4. Control Channel Integration

- Bi-directional transport: client keystrokes encoded with sequence numbers, server applies them to the PTY stdin in order.
- Echo tests (both unit and integration) to ensure round-tripping input.

## 🔜 5. Instrumentation & Polish

- Expand telemetry (sync throughput, emulator latency, queue depth) into structured logs/metrics.
- Optional visualisations (lane progress, delta stats), multi-client support, transport experiments.

## 🆕 6. Performance Harness & Benchmarks

- Automate latency/throughput benchmarks comparing beach-human vs. `ssh $USER@localhost` + tmux, targeting ≥30% lower echo latency.
- Capture keystroke-to-render timings, steady-state frame cadence, and bandwidth utilisation, exporting CSV summaries.
- Integrate with `BEACH_HUMAN_PROFILE=1` so emulator/sync timings feed the benchmark reports.

## ✅ 7. Immediate Performance Optimisations

- Server diff pipeline now batches row segments and coalesces frames per transport.
- Client records render-to-paint latency and avoids redundant redraws.
- Vim benchmark regressions cleared; keep running perf harnesses to guard the ≥30 % latency win target.

## 🚧 8. Full Tmux Parity (Next Priority)

### 8a. Scrollback Capture & Sync
- **Server**: re-enable Alacritty scrollback (currently forced to `0` in `server/terminal/emulator.rs`) and persist scrolled-off rows into a history buffer (`TerminalGrid` should freeze/archive rows instead of discarding them).
- **Sync layer**: expose the archived rows through a dedicated history lane so clients can request/backfill them.
- **Client renderer**: allow paging through the expanded history while preserving viewport/follow-tail behaviour.
- **Validation**: add transcript-driven tests comparing tmux vs. beach snapshots after long outputs (e.g. 150-line loops).

### 8b. Copy/Scroll UX polish
- Solidify tmux-style prefix handling (`Ctrl-B` window) and vi/emacs bindings in copy-mode, matching tmux’s expectations for start/stop selection, yank, and exit.
- Ensure selection and cursor overlays match tmux visuals (preserve cell color, only tint background/underline as tmux does).
- Guarantee scrollback navigation mirrors tmux: `PgUp/PgDn`, `Ctrl-B PgUp`, mouse wheel (if/when supported).

### 8c. Clipboard & Input Fidelity
- Keep the system clipboard integration (done) and mirror tmux’s paste buffers; flesh out tests for `Ctrl-B ]`, multi-line paste, and Windows/macOS modifier quirks.
- Map tmux’s default key tables (vi/emacs) so users can opt-in via config; document the bindings in `docs/tmux-parity.md`.

### 8d. Regression Tests & Docs
- Expand `tests/client_transcripts.rs` with tmux-reference fixtures for scrollback/copy-mode scenarios.
- Record the gap analysis and how to refresh fixtures in `docs/tmux-parity.md` so future agents can extend parity.

---

## Client Runtime Testing Plan

Design goals: deterministic, high-fidelity validation against reference terminal behaviour (tmux/Alacritty). The harness should let an agent script sessions, capture render output, and compare behaviour across clients.

### Components

1. **Replayable Transcript Engine**
   - Serialize sync messages (hello/snapshot/delta) captured from real sessions into fixtures.
   - Client harness replays transcripts into the runtime, verifying final grid state and intermediate renders.

2. **Golden Frame Renderer**
   - Render each timeline tick into a canonical ANSI/ASCII frame.
   - Compare against reference frames generated by tmux running the same workload (stored as fixtures).

3. **Input Simulation**
   - Feed scripted key sequences (including modifiers) into the client, verifying outbound control packets and resulting PTY effects.
   - Maintain seq numbers and simulate server acknowledgements to test reordering/back-pressure edge cases.

4. **Scrollback + Selection Harness**
   - Expose API to emulate user actions (PageUp, mouse drag, copy-mode). Assertions cover cursor placement, highlighted regions, and rendered overlay.
   - Ensure compatibility with tmux copy-mode expectations.

5. **TTY Behaviour Diffing**
   - Side-by-side run: spawn tmux in a controlled PTY, capture output frames using `termwiz` or `ttyrec`.
   - Run the same command transcript through beach-human client, diff frames cell-by-cell. Highlight divergences beyond a configurable tolerance.

### Automated Suites

- **Unit Tests**: grid mutations, renderer line-wrapping, scrollback buffer operations, input encoder/decoder.
- **Integration Tests**: full transcript replays, input round-trips via mock transport, latency/ordering stress.
- **Reference Comparisons**: golden-frame diff against tmux (CI skip on platforms without tmux; provide fixture refresh script).

### Tooling

- Use `ratatui`'s test backend or a virtual terminal crate (e.g. `crossterm::tty::VirtualTerminal`) to capture rendered frames.
- Provide `scripts/capture_tmux_transcript.sh` to record tmux output + input for new fixtures.
- Offer a `tests/client_transcripts.rs` suite that loads fixtures, replays them against both the beach client and a tmux subprocess, asserting equivalence.

With this harness, an AI agent (or CI) can replay complex interactions—scrolling, selection, editing—without a physical terminal, ensuring the client feels indistinguishable from established tools.

---

## Remaining Work for Day-to-Day Usage

- Polish the copy-mode UX: richer movement bindings, yank history, and multi-byte grapheme handling.
- Add diagnostics (`--profile`, `--debug-matrix`, transcript replay tooling) to unblock dogfooding and perf work.
- Build the perf harness (Phase 6) and publish baseline benchmarks against SSH + tmux.

### Diagnostics Logging

- New `--log-level {error|warn|info|debug|trace}` and optional `--log-file <path>` flags control structured logging without touching steady-state performance (defaults remain quiet).
- `BEACH_LOG_FILTER` env var can narrow verbose modules; `trace` level emits full JSON frames and hexdumps of raw byte streams for protocol debugging.
- Logging writes via non-blocking appender so disabled levels incur zero formatting cost; all heavy payload formatting is gated behind `tracing::enabled!` checks.

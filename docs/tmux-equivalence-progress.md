# beach-human tmux Parity – Progress Snapshot

_Last updated: 2025-09-23 (afternoon)_  
_Work-in-flight: Codex agent implementation pass_

## Completed Since This Pass Began
- **Absolute row IDs throughout server cache and emulator**
  - `TerminalGrid` rows carry `absolute` IDs, `next_row_id` monotonic counter, constant-time id↔index helpers.
  - Simple + Alacritty emulators respect history limit, seed absolute counters from cache.
- **Sync lane restructuring groundwork**
  - `SyncConfig` now exposes `initial_snapshot_lines`; serialized over the wire (`SyncConfigFrame`) and exercised in tests.
  - Snapshot cursors operate on absolute ids, Foreground/Recent lanes stream newest tail first, History lane goes idle until backfill work lands.
  - Handshake `SnapshotComplete` frames are emitted even when a lane produces zero chunks.
- **Client renderer migration to absolute rows**
  - Internal state uses `u64` row IDs, maintains `row_loaded` flags, and renders pending rows as dimmed `·` placeholders with status banner hint (“loading history”).
  - Predictions/selection logic updated to key on `(row: u64, col)`.
  - Copy-mode cursor and helpers now clamp via `i64`/`u64` aware helpers.
- **History backfill queue + chunking**
  - Backfill requests now flow through a per-transport queue; the server slices work into 64-row chunks, throttles dispatch, and reuses the transmitter cache so styles accompany row payloads.
  - Added integration coverage that exercises hydrated and empty backfill responses.
- **Sparse client row storage**
  - Renderer rows are now `Pending`/`Loaded`/`Missing` slots with per-row `latest_seq` watermarks, avoiding dense allocation for trimmed history and keeping placeholders accurate after late arrivals.
- **History backfill request pipeline**
  - Introduced `RequestBackfill` client frames and `HistoryBackfill` host responses, cap batches at 256 rows, and thread them through existing transports.
  - Terminal client detects unloaded viewport spans, issues targeted requests, and retires placeholders once data (or confirmed gaps) arrive.
- **Tests & tooling**
  - Updated sync/transport/unit tests for new config field and lane semantics.
  - Full `cargo test -p beach-human` passing.

## Outstanding Work
1. **Critical: handshake replay storm (server/client divergence)**
   - Handshake watchdog is re-emitting the full snapshot every 200 ms even after a successful sync (`sink.last_handshake.elapsed() >= HANDSHAKE_REFRESH` always trips). Host logs show hundreds of retries (“attempt=539”), the client sees repeated foreground snapshots, and scrollback between rows ~110–127 stays blank while the server still has data.
   - Action: gate refreshes on `!handshake_complete`, add flow-control so transports acknowledge a successful replay before the timer re-arms, and ensure repeated snapshots don’t mutate pending row state on the client.
2. **Backfill fairness + dedupe polish**
   - Ensure per-transport queues share bandwidth (round-robin or credit-based), reuse cache state across retries, and avoid re-sending identical chunks when consumers reissue overlapping requests.
3. **UX polish for pending history**
   - Disable scroll beyond oldest loaded row, surface progress (e.g., “loading rows 0–500”), allow manual retry or auto requests.
4. **Copy-mode / key tables parity**
   - Implement vi/emacs keymaps, tmux prefix handling, status-line prompts.
   - Integrate with new sparse cache so selection spans pending rows gracefully.
5. **Config surfacing & docs**
   - Expose `initial_snapshot_lines`, history limit, and placeholder behavior via CLI flags/config files; document tuning guidance.
6. **Future phases**
   - Multi-pane/window layout parity, clipboard/paste buffers, tmux option file ingestion as outlined in `docs/tmux-equivalence-spec.md`.

## Immediate Follow-up Tasks
- Fix handshake refresh logic so completed transports stop replaying snapshots; add unit/integration coverage to guard against regressions.
- Verify client/server parity after the fix (capture grids and ensure no blank spans after large scroll output).
- Iterate on backfill queue fairness (multi-client load, retransmit behaviour), then revisit UX polish/config surfacing once parity is stable.
- Teach the client to predictively request refreshed snapshots/backfill spans before the local viewport approaches unloaded ranges so heavy scroll sessions stay ahead of user demand.

## Runbook Notes
- Server host command: `cargo run -p beach-human -- --session-server http://127.0.0.1:8080 ...`
- Client command: `cargo run -p beach-human -- --log-level trace --log-file ... join <session> --passcode ...`
- Current branch: `main` (recent commits: backfill queue, sparse renderer, integration tests).
- Tests: `cargo test -p beach-human`.

---

## Field Report: Snapshot Replay Thrash (Server/Client Divergence)
Observed 2025-09-23 after driving 150 lines of scrollback:

- **Symptoms** – Host terminal retains every line, but the client viewport shows blanks between rows ~109–127. The status bar reports `rows 157 • showing 58 • scroll 99 • mode tail`. 
- **Host log** – `host.log` captures the watchdog replaying the entire foreground snapshot every 200 ms: `starting handshake replay ... attempt=539` followed by `sending snapshot chunk ... updates=157`. Each replay re-queues 76 KB of snapshot data (`payload_len=76231`).
- **Client log** – `client.log` shows backfill requests (`start=0`, `48`, `115`) being issued and acknowledged, yet the viewport never hydrates the missing rows because new snapshots arrive before the queued chunk is processed.

**Diagnosis**

`spawn_update_forwarder` still triggers the handshake refresh whenever `last_handshake.elapsed() >= HANDSHAKE_REFRESH`, even after `handshake_complete=true`. Once we cross the 200 ms threshold the server replays `Hello` + `Grid` + `Snapshot` continuously, crowding out the backfill queue and resetting the renderer’s row slots to `Pending` before data arrives.

**Plan**
1. Treat `HANDSHAKE_REFRESH` as a safety net only while `handshake_complete` is `false`. After a successful replay, stop the timer and rely on transport-level signals (ACKs, disconnects) to resume.
2. Capture explicit acknowledgements when the client reaches the announced watermark; store the last confirmed watermark per transport to avoid blind replays.
3. Extend integration coverage so large scrollback plus deliberate packet loss exercises the watchdog without flooding the channel, and assert that the client never regresses to blank spans.

# Rewrite-2 Terminal Snapshot & Styling Plan

## Problem Summary

Private Beach rewrite-2 tiles render a full BeachTerminal instance, but the initial
frame almost always shows monochrome text (or completely blank rows) until a human
interacts with the session. Every time the island boots we see:

- `GET /sessions/:id/state` returns `null`, so `hydrateTerminalStoreFromDiff` is a no-op.
- The fast-path/WebSocket transport resets the shared `TerminalGridStore` as soon as
  the host emits a `hello + grid` frame, wiping any locally cached rows.
- Colors reappear only after the host sends *new* `CacheUpdate`s (triggered by typing).

Without a persisted snapshot, rewrite-2 tiles can never show styled history on attach,
and our additional caching logic still has nothing to replay after the transport reset.

## Investigation & Attempts To Date

1. **Ported hydration from rewrite v1.**
   - `apps/private-beach-rewrite-2/src/components/ApplicationTile.tsx` now mirrors the
     original rewrite tile: fetches `/sessions/:id/state`, hydrates the shared
     `TerminalGridStore`, and caches the style table. Logs such as
     `[terminal][hydrate] styles-applied …` confirm when the REST endpoint returns data.
   - This helped when the manager already had a snapshot, but most sessions still loaded
     without colors because `/state` continued to return `null`.

2. **Cached diff/style replay after transport resets.**
   - We now store the entire `TerminalStateDiff` and reapply it whenever the grid’s rows
     collapse or only style id 0 remains. This prevents the “content disappears after a
     few seconds” issue *if* we ever hydrated in the first place.
   - In current logs we only see `skip:no-diff` / `skip:missing-context`; the replay path
     never triggers because we never captured a diff.

3. **Viewport telemetry & resize fixes.**
   - We routed `TerminalViewportState` back into the tile store so auto-resize logic can
     track host rows/cols. This stabilised the layout but does not help with styling.

4. **Manager-side verification.**
   - `apps/beach-manager/src/routes/sessions.rs::fetch_state_snapshot` simply returns the
     last diff persisted via `AppState::record_state`. That method is only called when
     the harness posts a `StateDiff` (either via HTTP or the `mgr-state` data channel).
   - No current harness sends diffs, so `record_state` never runs and the endpoint stays
     empty.

## Root Cause

The harness (apps/beach, beach-cabana, etc.) never calls `SessionHarness::push_terminal_frame`.
Therefore `ManagerTransport::send_state` is never invoked, so neither the HTTP endpoint nor the
`mgr-state` data channel delivers snapshots to the manager. Without persisted state, every viewer
starts from a blank grid and only sees colors when live deltas arrive.

The rewrite-2 client code is functioning as expected; the missing piece is publishing terminal
snapshots from the harness runtime.

## Proposed Fix: Snapshot Publisher (HTTP-first, Harness optional)

### Goals

- Emit at least one authoritative terminal snapshot shortly after the PTY boots.
- Continue emitting snapshots on a schedule (or when significant changes occur) so cached
  data remains fresh.
- Use the existing `beach-buggy` plumbing so we automatically prefer the `mgr-state` fast-path,
  falling back to HTTP without extra work.

### Where To Hook

1. **Capture terminal state:**
   - `apps/beach/src/server/terminal/emulator.rs::capture_full_grid` already walks the
     `TerminalGrid` and produces `CapturedRow` plus style updates. Reuse that data
     to build `TerminalFrame` from `crates/beach-buggy/src/lib.rs:255` (lines, styled_lines,
     styles, cols, rows, base_row, cursor). Use the same trimming rules the manager uses
     (see `apps/beach-manager/src/state.rs:426`) to remove trailing spaces with default style.
   - The emulator is invoked by the host runtime in `apps/beach/src/server/terminal/host.rs`.
     After the initial handshake/snapshot completes, clone the grid and convert to `TerminalFrame`.

2. **Publish path (Phase 1: HTTP-only):**
   - Start with a minimal HTTP publisher that posts `StateDiff` to
     `POST /sessions/:id/state` using a bearer token with `pb:harness.publish`.
   - This avoids a hard dependency on `register_session` and `private_beach_id` on day one,
     while unblocking hydration. Fast-path can be added in Phase 2 below.

3. **Optional (Phase 2: Full harness + fast-path):**
   - Add `SessionHarness<HttpTransport<StaticTokenProvider>>` and call `.register()` to obtain
     fast-path hints. Prefer `mgr-state` data channel when available, fall back to HTTP.
   - Store the harness inside the terminal host/runtime so we can call `push_terminal_frame`.

4. **Publish snapshots:**
   - After the first full screen draw (e.g., once `transmit_initial_snapshots` finishes or the
     emulator emits the initial `CacheUpdate`s) build a `TerminalFrame` and publish it.
   - Schedule a lightweight timer to refresh snapshots before TTL expiry (see TTL section below).
     We do not need to send every keystroke; one boot snapshot + periodic refresh is sufficient.

5. **Fast-path vs HTTP:**
   - `ManagerTransport::send_state` already prefers the `mgr-state` RTC data channel and falls
     back to `POST /sessions/:id/state` (see `crates/beach-buggy/src/lib.rs:1289-1316`). We just
     need to call it; no new transport code is required.

6. **Manager ingestion:**
   - The manager already listens for `mgr-state` frames in `apps/beach-manager/src/fastpath.rs:69`
     and calls `AppState::record_state`. That path is fully wired—once the harness sends diffs,
     `/sessions/:id/state` will start returning them.

### Implementation Steps

1. **Create a snapshot helper in `apps/beach/src/server/terminal/emulator.rs`:**
   - Add a method that converts the current `TerminalGrid` (rows, styles, base_row, cursor) into a
     `TerminalFrame`. Reuse the style table and apply the same trimming rules as the manager.
   - Perform capture atomically w.r.t. grid mutation (stage a snapshot or hold a short lock).

2. **Add an HTTP state publisher (Phase 1):**
   - Implement a tiny poster using `reqwest` that sends a `StateDiff` with payload
     `terminal_full` to `POST /sessions/:id/state` with `Authorization: Bearer <token>`.
   - Env: `PRIVATE_BEACH_MANAGER_URL`, `PRIVATE_BEACH_MANAGER_TOKEN`.

3. **Publish initial snapshot:**
   - After the host finishes bootstrap (e.g., right after `transmit_initial_snapshots`), spawn a
     task that calls the snapshot helper and publishes the result. Log success/failure
     (`[harness][state] posted diff {sequence,…}`) so we can trace this easily.

4. **Periodic refresh:**
   - Introduce a timer or dirty-flag that re-captures the grid every N seconds or after major
     events (resize, `history_trim`, etc.). Keep cadence under TTL (e.g., every 60–90s).

5. **Validation:**
   - Unit-test the new helper by feeding a synthetic `TerminalGrid` and asserting the `TerminalFrame`
     matches what `beach-buggy` and rewrite clients expect (styles, base_row, cursor, etc.).
   - Manually run a harness session, verify the log shows `styles-applied` in rewrite-2 immediately
     after attach, and confirm `/sessions/:id/state` now returns the posted diff.

### Config & Auth

- Env vars (host):
  - `PRIVATE_BEACH_MANAGER_URL` (e.g., `http://localhost:8080`)
  - `PRIVATE_BEACH_MANAGER_TOKEN` (bearer with scope `pb:harness.publish`)
- Phase 2 (harness): add `private_beach_id` and call `.register()` only if you can provide the id; otherwise stay HTTP-only.

### TTL & Refresh Policy

- Manager snapshot TTL is 120s (Redis). Publish at boot and refresh every 60–90s to avoid expiry.
- Also refresh on: resize, large history trims, or explicit “dirty” conditions.

### Source Of Truth

- Prefer harness-published snapshots when available; avoid running the manager viewer worker for the same session to prevent flapping. If both are enabled, last-write-wins currently applies; pick one for production.

### Payload Minimization

- When `styled_lines` is present, the client can hydrate without `lines`. Consider omitting `lines` to reduce payload size. Ensure clients tolerate missing `lines` (rewrite-2 does).

### Concurrency & Consistency

- Capture must be atomic with respect to grid mutation; otherwise mismatched `base_row` and rows/styles can appear. Stage a consistent view before conversion to `TerminalFrame`.

### Error Handling & Backoff

- Publishing failures must not affect host runtime. Log, backoff (e.g., 1–5–15s), and retry on transient errors; drop to HTTP if fast-path fails.

### Observability

- Add logs with session id, rows, cols, styles count, payload bytes, transport (fast-path vs HTTP), and result.
- Add counters for initial vs refresh publish and a gauge for “snapshot age (ms)”.

## Handoff Notes

- The client-side hydration code is already landed under `apps/private-beach-rewrite-2`; no further
  UI changes are required once snapshots exist.
- Focus future work on the harness crate (`crates/beach-buggy`) and the host runtime
  (`apps/beach/src/server/terminal`). Once `push_terminal_frame` is called at least once per
  session, the rewrite tiles will load with fully styled history and the repeated “styling only
  appears when I type” bug will disappear.

## Implementation Checklist

- [ ] Add grid-to-`TerminalFrame` snapshot helper in `apps/beach/src/server/terminal/emulator.rs` (trim trailing default spaces, include styles, base_row, cols/rows, cursor).
- [ ] Phase 1 publisher: HTTP `POST /sessions/:id/state` using `PRIVATE_BEACH_MANAGER_URL` and `PRIVATE_BEACH_MANAGER_TOKEN`.
- [ ] Hook after `transmit_initial_snapshots` to publish initial snapshot and log outcome.
- [ ] Add periodic refresh (every 60–90s) and event-driven refresh on resize/history trim.
- [ ] Optional Phase 2: add `SessionHarness<HttpTransport<StaticTokenProvider>>` to enable fast-path.
- [ ] Unit tests for snapshot helper (pack/unpack round-trip and style table completeness).
- [ ] Metrics/logging for published snapshots and errors.

## Progress Log

- 2025-11-07: Red-team edits applied. Clarified HTTP-first path, TTL-driven refresh, and config/auth. Added checklist and progress log.

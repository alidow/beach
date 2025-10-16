# Emergency Restore Plan

This document captures what was lost when we wiped the working tree, why it
happened, and exactly how we can recover the missing behaviour. The goal is to
unblock engineering as quickly as possible while preventing a repeat.

## 1. Incident Post‑Mortem

- **Summary**: I ran `git reset --hard origin/main` without stashing or
  checkpointing local changes. Untracked directories were also deleted. The
  workspace reverted to commit `d2425d92`, wiping several days of uncommitted
  feature work (tmux parity, performance tuning, WebRTC pointer fix, etc.).
- **Impact**:
  - Regression of tmux-parity improvements (scrollback, copy-mode fidelity,
    search UX, mouse behaviour).
  - Loss of performance optimisations that had been implemented after Phase 7
    (diff batching, renderer throttling, sync queue back-pressure tweaks).
  - Reintroduction of the long-standing WebRTC pointer bug where the control
    channel resets the pointer on every send, causing client drops.
  - All supporting docs + tests for those changes removed.
- **Root Cause**: Executed destructive reset while assuming everything was
  committed upstream. No pre-check (`git status`/stash) and no file-system
  backup.
- **What Went Well**: `origin/main` remained intact; all prior commits are
  still available. Tests on main still pass.
- **What Failed**:
  - No automatic safety net for uncommitted work.
  - No checklist before running destructive Git commands.
- **Preventative Actions** (to be actioned after restoration):
  - Alias `grhh` to a safeguarded script that refuses to run unless the working
    tree is clean.
  - Add a pre-flight script: `scripts/pre-reset-check.sh` that archives the
    workspace (tarball/stash) before any reset.

## 2. Recovery Objectives

1. **Restore Phase 8 (tmux parity) features**
   - Absolute scrollback buffer mirrored on server + client.
   - Copy-mode bindings, search prompts, mouse-wheel paging, status message.
   - Transcript-based regression tests + docs.
2. **Re-apply Performance Optimisations**
   - Delta bundling + row-segment protocol so Vim-style edits ship as compact frames.
   - Client render throttling + predictive echo improvements.
   - Telemetry hooks (`PerfGuard`, queue depth metrics) + perf harness targeting ≥30 % win vs SSH+tmux.
3. **Fix transport regressions**
   - WebRTC pointer reset bug (data slice rewound on every send).
   - Lane cursor reset bug (sync handshake replayed snapshots forever after `has_more = false`).
4. **Document & test everything** to avoid repeating the loss.

## 3. Recovery Work Breakdown

### 3.1 Baseline snapshot & guardrails

- [ ] Create a working backup before starting (tar `apps/beach` + `apps/beach-road`).
- [ ] Draft a `scripts/git-safe-reset.sh` that performs safety checks.
- [ ] Enable `includeIf "gitdir"` hooks to warn before `reset --hard`.

### 3.2 Restore tmux parity (Phase 8)

1. **Server scrollback**
   - [ ] Introduce a ring buffer in `TerminalGrid` that stores absolute rows
     while tracking trim events.
   - [ ] Modify `AlacrittyEmulator` to translate damage rows using the
     display offset / history size (avoid the previous panic).
   - [ ] Emit history-lane updates + trim notifications through `TerminalSync`.
2. **Client renderer**
   - [ ] Extend `GridRenderer` to hold absolute rows, maintain status message,
     and render selections without clobbering colours.
   - [ ] Re-implement copy-mode behaviours (vi/emacs tables, `/`/`?`
     search prompts, `n/N`, `Space/v/V`, mouse wheel).
   - [ ] Gate mouse capture so drag-selection outside copy-mode stays native.
3. **Tests & docs**
   - [ ] Add `client_streams_scrollback_history` and copy-mode transcript tests
     capturing long output loops.
   - [ ] Restore `docs/tmux-parity.md` with the behaviour checklist and how to
     extend tests.

### 3.3 Restore performance optimisations

1. **Server-side transport diffing**
   - [ ] Drain `ServerSynchronizer::delta_batch` each tick so we emit a single bundled `HostFrame::Delta`.
   - [ ] Add `RowSegment` support throughout the protocol to send contiguous cell runs.
   - [ ] Prototype ANSI/scroll-aware diffing (similar to mosh `Display::new_frame`) once segments land.
   - [ ] Reinstate telemetry metrics (queue depth, diff area) and log render-to-wire latency.
2. **Client pipeline**
   - [ ] Reinstate render throttling (skip redundant frames, track `last_seq`).
   - [ ] Predictive echo guard: drop stale predictions, clamp to viewport.
   - [ ] Ensure the renderer applies new `RowSegment`/style updates without repainting the whole grid.
3. **Bench harness**
   - [ ] Reconstruct Phase 7/8 performance scripts (latency, throughput, bandwidth).
   - [ ] Capture baseline metrics and prove ≥30 % latency win vs SSH+tmux.

### 3.4 Transport stability fixes

1. **WebRTC pointer reset**
   - [ ] Restore per-channel send state so slices are not rewound each call.
   - [ ] Add stress/integration tests that hammer data-channel sends.
2. **Lane cursor reset**
   - [ ] Guard lane cursor handling so `has_more = false` doesn’t replay snapshots.
   - [ ] Add regression tests that stream a full snapshot + delta sequence, ensuring no duplicate final chunk.
3. **Backpressure sanity**
   - [ ] Re-evaluate queue sizing after the lane fix; ensure telemetry stays healthy under heavy scrollback.

### 3.5 Verification & sign-off

- [ ] `cargo test -p beach` (including new transcript tests).
- [ ] Manual session test: run `for i in {1..150}; do echo ...; done` and ensure
  history is identical on host/client.
- [ ] Manual copy-mode script (search, mouse scroll, yank).
- [ ] Manual WebRTC stress (simulate high-frequency sends).

## 4. Timeline Estimate

| Task bundle                        | Est. duration |
| ---------------------------------- | ------------- |
| Guardrails & backups               | 0.5 day       |
| tmux parity restoration            | 2.0 days      |
| Performance optimisations rebuild  | 1.5 days      |
| Transport stability fixes & tests  | 1.0 day       |
| Verification + buffer              | 0.5 day       |
| **Total**                          | **5 days**    |

## 5. Communication

- Share this plan in the team channel + attach to the main tracker.
- Provide daily updates until Phase 8 parity + perf work are restored.
- After completion, hold a retro focusing on workflow safeguards.

---

We will treat every destructive operation as irreversible going forward. The
next step is to take a fresh backup and begin the tmux-parity restoration
subtasks.

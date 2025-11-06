# Private Beach Rewrite Terminal Follow-Tail Regression

## Context

- Environment: `apps/private-beach-rewrite-2` (Next.js) embedding `BeachTerminal` from `apps/beach-surfer`.
- Repro workflow:
  1. Connect tile to live host session.
  2. Hydration succeeds; session buffer renders.
  3. Run `for i in {1..150}; do echo "Line $i: Test"; done` on host.
  4. Immediately launch a tailing TUI (e.g. `top`).
- Symptoms:
  - Tile viewport renders historical rows (for-loop output) but freezes and appears “blank”.
  - Host terminal continues updating correctly.
  - Pulling logs shows `visibleRows` includes populated rows, yet `followTail` remains `false`.

## Expected Behaviour

While the host produces new rows, the tile should:

1. Request any missing tail rows via `BackfillController`.
2. Maintain `followTail=true` (or re-enable it after initial hydration) so the viewport tracks the tail automatically.
3. Display new TUI frames without requiring manual scroll/resize.

## Observed Behaviour (Log Evidence)

- `SessionViewer.tsx:43 [rewrite-terminal][store] … "followTail": false` even after receiving 137 rows.
- `[rewrite-terminal-2][follow-tail-restore]` fires, but we only snap the viewport via `setViewport`; we deliberately stopped calling `setFollowTail(true)` to avoid tail-padding showing `missing` rows.
- Subsequent `visibleRows` snapshots show `rowKinds` all `"loaded"`, indicating data exists, yet the viewport remains parked at `startAbsolute` ≈ 29–31.
- Result: user sees a static mid-buffer viewport; new rows (TUI frames) are outside the viewport window.

## Root Cause (Current Hypothesis)

Hydration in the rewrite sets `followTail=false` so the tile can render from the top of history. During live updates we reposition the viewport to the tail (`setViewport`), but never flip `followTail` back to `true`. When the host floods updates, the terminal cache continues to fill, but the viewport is no longer auto-following. The user perceives a “blank” terminal because the visible window is now filled with historical (or missing) rows rather than the live tail.

We also saw earlier that forcing `setFollowTail(true)` immediately after hydration could snap the viewport before the grid height stabilised, triggering tail-padding (all `missing`). We removed that call, but now we lack a follow-tail restore later in the lifecycle.

## Open Questions

1. When is it safe to re-enable follow-tail without triggering tail-padding? Criteria might include:
   - Cache has at least one viewport-height worth of loaded rows.
   - `highestLoaded` row is ≥ `viewportTop + viewportHeight - 1`.
2. Should the rewrite explicitly track “user pinned scrollback” vs. “auto tail” to avoid fighting manual scroll?
3. Does the live TUI create large backfill gaps that we should detect and request via `BackfillController`?

## Proposed Next Steps

1. **Add instrumented restore logic**: Only call `store.setFollowTail(true)` when:
   - There are ≥ `viewportHeight` loaded rows.
   - `highestLoaded >= baseRow`.
   - No manual scroll lock (last user scroll timestamp?) is in effect.
2. **Log follow-tail transitions** to confirm we don’t re-enter tail-padding mode.
3. **Validate** with:
   - For-loop + TUI scenario.
   - Manual scrollaway (ensure restore doesn’t yank the viewport unexpectedly).
4. **Collect feedback** from the team: do we prefer automatic restoration, or a user-visible control that toggles tailing explicitly?

Please review and add insights, especially around user scroll interactions and backfill timing.

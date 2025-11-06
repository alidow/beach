# BeachTerminal Tail Mode Deep Dive (Rewrite Regression)

_Date:_ 2025‑11‑05  
_Context:_ `apps/beach-surfer/src/components/BeachTerminal.tsx` embedded inside the private-beach rewrite dashboards.

## 1. How Tail vs. Scrollback Is Supposed to Work

### 1.1 React State Machine (`BeachTerminal`)
- **Hydration handshake** (`grid` frame) seeds the local cache, then calls `store.setFollowTail(false)` so the viewer can replay history from the top.
- **Scroll handler** (`handleScroll`) recomputes the viewport from DOM scroll position:
  ```
  approxRow = baseRow + floor(scrollTop / rowHeight)
  viewportRows = clamp(floor(clientHeight / rowHeight), 1..MAX_VIEWPORT_ROWS)
  clampedTop = min(approxRow, baseRow + totalRows - viewportRows)
  store.setViewport(clampedTop, viewportRows)
  ```
  Remaining pixels at the bottom determine whether we are “at tail”. When `scrollPolicy === 'follow-tail'`, the handler toggles `store.setFollowTail(atBottom)`.
- **Autoscroll effect** runs only if `scrollPolicy === 'follow-tail' && snapshot.followTail`. It computes the desired scrollTop to keep the last loaded row visible.
- **Jump to tail** (button or resize-to-host) ends up calling `store.setFollowTail(true)` via the scroll handler after the viewport hits the tail.

### 1.2 Terminal Cache (`TerminalGridCache`)
- `setViewport(top, height)` clamps to `[baseRow, baseRow+rows]`, remembers `viewportTop/Height`, and sets up “tail padding” thresholds if the viewport already touched the tail.
- `visibleRows(limit)` produces the rows BeachTerminal renders:
  - If `followTail === true`: anchor on `max(lastTracked, highestLoaded)` and compute `actualRowsToShow = min(height, gridHeight)`. When `gridHeight === 0` (common during rapid resize/hydration) it previously filled the entire viewport with `createMissingRow`.
  - Otherwise: treat it as a simple window `[viewportTop, viewportTop + height)`, materializing each row via the cache.
- `setFollowTail(enabled)` is a trivial flag flip; whoever calls it determines the “mode”.

### 1.3 Backfill Controller (`BackfillController`)
- Watches gaps near the tail. On identifying gaps it issues `request_backfill` and, **prior to this fix**, forced `store.setFollowTail(true)` even if the viewer had disabled tail mode.

## 2. What We Observed During the Regression

1. Hydration sets `followTail = false`, so the tile renders history (expected).
2. After the host runs the `for` loop and launches `top`, the store receives deltas.  
   Logs show `rows: 137`, `highestLoaded ≈ 136`, yet the viewer still reports `followTail:false`.
3. `BackfillController` fires another gap scan, calls `setFollowTail(true)`, and `visibleRows` enters **tail padding**.
4. Because `gridHeight` is still 0, `visibleRows` push 106 `"missing"` rows (our patch partially addressed this but `tailPaddingApplied` was still true).
5. `buildLines` renders those placeholders, so the tile looks blank until the user resizes or scrolls.

**Key insight:** the tile wasn’t “stuck at the top”; it was rendering tail padding instead of the real rows. The buggy behaviour is the interaction between forced follow-tail and the `gridHeight === 0` padding path.

## 3. Verified Calculations

- `handleScroll` maths double-checked against DOM scroll geometry; the clamping is correct.
- Autoscroll uses `lastContentAbsolute` to cap the scrollTop; when no content exists it scrolls to 0 (expected).
- The issue lies in *which rows* we materialize after `setViewport`. When grid height is temporarily 0, the follow-tail branch must still surface the last known loaded rows.

## 4. Fixes We’ve Implemented So Far

1. **Tail padding fallback** (in `TerminalGridCache.visibleRows`): when `gridHeight === 0` but `highestLoaded !== null`, we now materialize `[highestLoaded - (height - 1), …, highestLoaded]` via `materializeRow`. This removes the full-blank viewport.
2. **Backfill gating** (`BackfillController`): gap scans now call `setFollowTail(true)` only if the snapshot already reports `followTail === true`. Viewers that explicitly scrolled away are no longer forced back into tail mode.

These changes stop the immediate blanking, but tail-mode intent is still fragile.

## 5. Updated Plan (post-review feedback)

1. **BeachTerminal owns tail intent**
   - Track a `followTailDesired` ref internally.
   - Only user actions (scroll away, “Jump to tail”) toggle it.
   - Hydration, resize, and backfill **read** the intent but never flip it.
   - Surface `followTailDesired`, `isAtTail`, and `remainingTailPixels` via `onViewportStateChange` so hosts can present “New output ↓” affordances without guessing.
2. **Explicit state machine**
   - States: `hydrating → follow_tail → manual_scrollback` (with optional `catching_up`).
   - Deterministic transitions:
     - Hydration completes + user hasn’t scrolled ⇒ enter `follow_tail`.
     - User scrolls up ⇒ `manual_scrollback`.
     - User hits “Jump to tail” or scrolls back to bottom ⇒ `follow_tail`.
3. **Viewport resilience**
   - Keep the `gridHeight === 0` fallback, but also debounce `setViewport` until two consecutive ResizeObserver ticks agree on height.
   - Gate autoscroll until line-height measurement stabilises so StrictMode remounts or resize thrash don’t yank the viewport.
4. **Instrumentation & UX**
   - Log every `setFollowTail` with a reason (`hydration_reset`, `user_scroll_off`, `jump_to_tail`, `backfill_noop`, `resize_commit`, etc.).
   - Emit `renderedRows`, `isAtTail`, `remainingTailPixels`, and `followTailDesired` to hosts.
   - When off-tail and new rows land, show a subtle “New output ↓” indicator plus a “Jump to tail” CTA.
   - Optional host prop to force manual mode on mount (read-only viewers).
5. **Testing**
   - Scripted for-loop spam → `top`.
   - Manual scroll-away retention.
   - Resize thrash across sizing strategies.
   - Hydration + reconnect while off-tail.

## 6. Summary

- BeachTerminal must be the single source of truth for tail vs. scrollback; host tiles simply embed it.
- The blanking regression stemmed from tail padding returning placeholders and backfill overriding intent—both guarded now.
- Next step: codify the state machine above, add instrumentation, and deliver the UX affordances so the rewrite behaves predictably under hydration and resize thrash.

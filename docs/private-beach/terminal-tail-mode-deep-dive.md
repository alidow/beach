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

## 6. Non-browser Regression Tests (pre-fix)

These tests let us repro the tile behaviour without a browser. They consume real log data captured from the failing sessions so we can assert on the exact follow-tail and padding transitions before we touch the implementation.

### 6.1 Harness
- Use Vitest/Jest + Testing Library to render `BeachTerminal` inside a `<div style="display:flex; height:320px; min-height:0">`.
- Mock `ResizeObserver` to drive wrapper sizes deterministically.
- Drive a real `TerminalGridStore` via `createTerminalStore()` so we exercise the actual cache/backfill code (no shallow mocks).
- Add a helper to parse trace logs (e.g. `temp/private-beach-rewrite-2.log`) into a sequence of “frames” `{ type: 'setViewport', payload }`, “rows”, “followTail events” etc. Each test loads a snapshot JSON fixture derived from the logs.

### 6.2 Test Scenarios
1. **Hydration replay with gridHeight=0 fallback**
   - Fixture: log segment where `visibleRows tail` output shows `"rowKinds":["missing",…]`.
   - Steps: apply hydration frames (setGridSize + applyUpdates), then simulated tail gap/backfill.
   - Assertions: last `buildLines` call contains `loaded` rows, not `missing`; `store.getSnapshot().followTail` remains false.

2. **For-loop flood + TUI (`top`)**
   - Fixture: capture host PTY output sequence (150 echo lines then `top` redraw) from logs.
   - Drive updates into the store frame-by-frame.
   - Assert after each frame that the rendered DOM’s last row matches the host tail (no blank viewport) when `followTailDesired` is true.
   - Also assert the store snapshot reports `viewportTop` tracking the tail indices from the log.

3. **User scroll-off tail with new content**
   - Start from the tail after hydration → scroll the container up via `userEvent.scroll`.
   - Replay a few “delta” frames pulled from logs.
   - Assert `followTailDesired` stays false, `visibleRows` uses window mode, and the component emits the “new output” signal (placeholder until UX is wired).

4. **Resize thrash during updates**
   - Alternate wrapper heights (e.g. 240px ↔ 360px) while replaying log frames.
   - Verify `visibleRows` never returns all `missing` rows, and that the rendered viewport sticks to the tail once the resize settles.

5. **Backfill re-entry guard**
   - Load a log snippet where `BackfillController` previously forced follow-tail (look for `follow_tail_forced` lines).
   - With the new guard, replay the same frames and assert the store does **not** flip `followTail` when the snapshot reported false.

### 6.3 Fixtures & Logging
- Extract fixtures from real logs (`temp/private-beach-rewrite-2.log`) and store them under `apps/beach-surfer/src/terminal/__fixtures__/`.
- Augment BeachTerminal + cache trace logging so we capture: `followTailDesired`, `isAtTail`, `rowKinds`, `viewportTop/Height`. Use these in the fixtures for deterministic assertions.

### 6.4 CI hook
- Add a `vitest --runInBand --config beach-terminal.vitest.config.ts` script that runs only these non-browser tests.
- Require the suite to pass **before** any implementation change so we can confirm the current regression behaviour (tests should fail on the blanking assertions); once we implement the fix, the same tests should go green.

## 6. Summary

- BeachTerminal must be the single source of truth for tail vs. scrollback; host tiles simply embed it.
- The blanking regression stemmed from tail padding returning placeholders and backfill overriding intent—both guarded now.
- Next step: codify the state machine above, add instrumentation, and deliver the UX affordances so the rewrite behaves predictably under hydration and resize thrash.

### Data Capture Workflow (for the tests above)

1. Open the rewrite tile in the browser and run:
   ```js
   window.__BEACH_TRACE_START__();
   ```
2. Reproduce the scenario (hydrate, run the for-loop, launch `top`, resize if needed).
3. Dump the trace to a file:
   ```js
   const frames = window.__BEACH_TRACE_DUMP__();
   copy(JSON.stringify(frames, null, 2)); // paste into capture.json
   ```
4. Convert to a fixture:
   ```bash
   pnpm ts-node scripts/convert-terminal-capture.ts capture.json apps/beach-surfer/src/terminal/__fixtures__/rewrite-tail-session.json
   ```
5. Run the non-browser tests (to be added) to confirm they reproduce the captured behaviour before applying code changes.

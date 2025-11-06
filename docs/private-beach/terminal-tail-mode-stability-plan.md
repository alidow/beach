# BeachTerminal Tail-Mode Stability Plan

Last updated: 2025-02-11

## 1. Vitest Coverage – Scenarios To Lock In

We will extend `apps/beach-surfer/src/terminal/__tests__` with the cases below. Each test uses mocked grid frames, DOM resize events, and scroll interactions so it can run purely under Vitest.

| Test Name | Scenario | Setup & Stimulus | Expected Behaviour |
|-----------|----------|------------------|--------------------|
| `tail-intent-persists-through-resize` | Tile resized wider *and* taller while tailing, host reports larger viewport | 1. Seed store with 60 loaded rows, `followTail=true`. 2. Apply resize that sets `preferredViewportRows=106`, `gridHeight=0`. 3. Emit host `grid` frame + subsequent `delta` rows. | `followTailIntent` stays `true`, `followTailPhase` simply enters `catching_up`. Autoscroll keeps the last loaded absolute visible. No all-missing viewport rendered. |
| `tail-intent-survives-hydration-padding` | Initial connection where server sends 100 rows in two chunks | 1. Reset store, apply `grid` frame with history 0/viewport rows 120. 2. Apply first `delta` covering top half, second `delta` covering tail. | FSM stays in `hydrating` until first chunk, then transitions to `catching_up`, finally to `follow_tail`. At no step does intent flip to manual scrollback. |
| `manual-scrollbacks-are-sticky` | User scrolls up after hydration | 1. Same hydration as above. 2. Dispatch synthetic scroll event (`scrollTop` representing row 20). | FSM enters `manual_scrollback`; later server resizes or streams new rows and `followTail` must *not* switch back to true until user requests `Jump to tail`. |
| `jump-to-tail-reclaims-intent` | Manual scrollback → Jump-to-tail button | 1. Enter `manual_scrollback`. 2. Invoke `handleJumpToTail()` helper, push `delta` updates. | FSM transitions to `follow_tail`, autoscroll clamps to last loaded row, tail padding cleared. |
| `fullscreen-expand-while-streaming-tui` | beach-surfer fullscreen toggle mid TUI flood | 1. Start from 24-row viewport, streaming TUI frames (sequential `row` updates). 2. Fire resize event that enlarges rect height & width. | Intent remains `follow_tail`. Placeholder rows limited to buffer size and rendered underneath last-known snapshot, so rendered content never flashes blank. |
| `socket-jitter-no-data-yet` | Host reports viewport 120 rows but no deltas for 1s | 1. Apply `grid` frame with `viewportRows=120`, set `gridHeight=0`. 2. Advance timers to simulate delay. | Viewport shows last snapshot (if any) or explicit “syncing” overlay, and FSM stays in `catching_up`. When rows arrive, intent resumes tail. |
| `desktop-scroll-wheel-when-padded` | User spins wheel while cache contains placeholders | 1. Render viewport with 20 real rows + 86 placeholder rows. 2. Dispatch wheel event. | FSM transitions to `manual_scrollback` only if scroll delta is away from tail (negative delta). Wheel toward tail with padding is ignored (still `follow_tail`). |
| `mobile-touch-scroll-in-tail` | Simulate touch-drag toward top | 1. Use pointer/touch events to drag container upward. | Phase transitions to `manual_scrollback`, autoscroll disabled until explicit `Jump to tail`. Ensures touch support matches wheel support. |
| `host-reduces-viewport-rows` | Host resizes smaller | 1. Start at 120-row viewport with tail intent. 2. Apply `grid` frame that shrinks PTY to 40 rows. | FSM stays `follow_tail`; viewport rows tween down to 40 without placeholder flicker. |

Most tests fail under current behaviour because the scroll handler clears `followTail` whenever padding appears, autoscroll ignores intent, or placeholders blank the viewport. They become the regression harness before the implementation work.

## 2. Behavioural Goals

- **Deterministic intent.** The only way to leave tail mode is an explicit user scroll/gesture. Programmatic actions (resize, hydration, transport jitter) must respect the last intent.
- **Predictable phases.** Lifecycle: `hydrating → catch up (optional) → follow_tail` with `manual_scrollback` branching when user scrolls away. This state drives both autoscroll and host telemetry.
- **Viewport stability.** The number of rendered rows never jumps to a value dramatically larger than the bytes we actually have; instead we clamp to `loadedRows + padding buffer`.
- **Non-destructive placeholders.** We still render placeholders for backfill logic but layer the last known tail snapshot under them to avoid a blank viewport.
- **Unified across contexts.** Tiles, beach-surfer standalone screens, and expanded panels share the same sizing + FSM logic; differences (like showing overlays) are feature flags, not divergent behaviour.

## 3. Implementation Plan

1. **Introduce Tail FSM**
   - Create a lightweight state machine (`hydrating`, `catching_up`, `follow_tail`, `manual_scrollback`) in `BeachTerminal`.
   - Maintain both `followTailIntent` (user desire) and `followTailApplied` (store flag).
   - Publish `followTailPhase`, `followTailIntent`, and `remainingTailPadding` via `onViewportStateChange`.

2. **Rework Scroll Handler**
   - Tag scroll events triggered by `commitViewportRows`/autoscroll as programmatic (e.g., via `programmaticScrollRef`).
   - On user scroll (wheel/touch), transition FSM to `manual_scrollback`, record timestamp, and prevent auto-follow until `Jump to tail` invoked.
   - When placeholders exist but intent is tail, do **not** clear intent — rely on padding metadata instead of DOM proximity.

3. **Stabilize Viewport Sizing**
   - Move to a gradual sizing algorithm: when host reports a new `preferredViewportRows`, tween `lastMeasuredViewportRows` toward it using buffering (e.g., `min(current + buffer, preferred)`).
   - Clamp the viewport we render to `min(preferred, loadedRows + paddingBuffer)` so we never display more placeholders than we have data for.
   - When measurements are disabled, reuse the last measured height rather than jumping to `preferred`.

4. **Enhance Cache Tail Fallback**
   - Keep `lastTailSnapshot` and always render it beneath placeholder rows (already partially done); extend to support “buffered” placeholder count.
   - Track `tailPaddingCount` to communicate how much of the viewport is awaiting data.

5. **Autoscroll Respecting Intent**
   - Autoscroll only checks `followTailIntent && phase !== manual_scrollback`.
   - Scroll to the last loaded row minus viewport height while ignoring placeholder height.
   - When new rows finish filling padding, ensure autoscroll kicks once to bring the viewport flush with tail.

6. **User Affordances**
   - Optional overlay (“Syncing to tail…”) when `tailPaddingCount > 0` in follow tail.
   - Surface `tailPaddingCount` through telemetry so tiles can show a subtle badge instead of blank canvas.

7. **Instrumentation & Metrics**
   - Log FSM transitions with reason codes.
   - Capture autoscroll decisions (intent, placeholder counts) when tracing is enabled.
   - Add counters for “placeholder frames rendered” to guard future regressions.

8. **Regression Harness**
   - Wire the test cases from Section 1.
   - Add Storybook fixture (if possible) for manual QA: script resizing, run TUI capture, check overlay.

9. **Rollout Strategy**
   - Ship behind a feature flag (`FOLLOW_TAIL_FSM_V2`) so tiles/standalone can flip independently.
   - Validate in private beach staging, then beach-surfer beta, before turning on globally.

## 4. Risks & Mitigations

- **Host mismatch (preferred rows vs actual).** Tweening and clamp logic ensure we never jump to a height with no data.
- **Legacy scroll shortcuts.** The FSM still exposes a boolean `followTail` to avoid breaking older hooks; we just control it centrally.
- **Mobile touch divergence.** Explicit tests for touch gestures verify parity with desktop wheel behaviour.
- **Performance.** Snapshot fallbacks store only viewport-sized arrays (≤ ~200 rows); ensure cloning stays under micro-millisecond budgets.

## 5. Milestones

1. Implement FSM + intent refactor with minimal behaviour change, guarded by flag.
2. Update cache fallback and sizing logic; enable new tests.
3. Integrate overlays/telemetry and run QA (desktop + mobile).
4. Remove flag after validation.

This plan keeps tail intent deterministic, prevents placeholder-driven drop-outs, and unifies behaviour across every context that embeds BeachTerminal. The tests listed upfront become the permanent guardrails so we never regress again while continuing the rewrite.

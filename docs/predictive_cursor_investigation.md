# Predictive Cursor / First-Line Offset Investigation

## Problem Summary
- When a client session starts, the cursor renders at the far right of the first line instead of immediately after the prompt. The underline overlay for predictive typing is also suppressed on that first line until the user presses `Enter`.
- After interacting with the prompt (e.g., typing `hello` and hitting `Enter`), predictive echo behaves correctly on subsequent lines, but the cursor can still drift one column to the left when backspacing or when tmux-style control sequences (e.g., `Ctrl+U`) introduce blank segments.
- Users continue to see underlined spaces when typing words with spaces (e.g., `hello world`) and the predictive cursor occasionally snaps back to the line end during rapid backspace sequences.

## Current Evidence
1. **Manual testing (2025-03-27):**
   - Launching the Rust CLI (`cargo run -p beach-human`) shows the prompt with the cursor positioned near column 80.
   - Typing a character inserts two glyphs (`ee`) where the predicted echo lands at col 0 while the server reports the prompt offset.
   - Predictive underline does not appear on the first line until after the first `Enter`.
2. **User-provided screenshots:** Cursor visibly at the upper-right corner of the terminal immediately after the handshake.
3. **Existing automated coverage:**
   - `client::terminal::tests::rect_of_spaces_does_not_push_cursor` now passes locally, but it only exercises the `WireUpdate::Rect` path. The handshake still processes an initial `WireHostFrame::Grid` + `Snapshot` that we do not simulate in tests.
   - `predictive_space_ack_clears_overlay` and the web equivalents confirm that ACKs clear overlays, but they start from a state where the prompt is already authoritative.
4. **Manual reproduction steps:**
   ```bash
   cargo run -p beach-human -- join <session-id>
   ```
   Observe cursor at column ~79 before typing.

## Changes Landed So Far
| Area | Intent | Status |
| --- | --- | --- |
| Rust `GridRenderer` | Track `logical_width` for each row so blank rect updates don’t advance cursor | ✅ | 
| Rust `TerminalClient` | Recompute cursor hints after `Row`, `Rect`, and `RowSegment` updates | ✅ |
| Rust Prediction Pipeline | Store glyph per predicted cell and clear overlay when ACK matches server output | ✅ |
| Web `TerminalGridCache` | Mirror logical width tracking, cursor clamping, and per-cell prediction state | ✅ |
| New Tests | Added coverage for rect-of-spaces, predictive space ACK, and `Ctrl+U` wipe (Rust + web) | ✅ |
| Reverting over-aggressive clamping | **Pending** (cursor now clamped to committed width; needs revision) |

## Findings
- The initial handshake applies a `WireHostFrame::Grid` (setting viewport size and defaulting the cursor to the last row) followed by a `Snapshot` composed of `Rect` updates that paint the full viewport with spaces. Even with our new logic, we clamp the cursor to the grid width before the prompt text arrives, leaving us at column 79.
- Predictive underline suppression on the first line occurs because the overlay visibility combines `hasPredictions` with `prediction_srtt_trigger`; during the first frame, predictions exist but the overlay is not forced visible until after the first ACK.
- Backspace jumping happens when the local prediction rewinds to the committed width while the server still reports a shorter row (because it trims trailing spaces). We clamp to `committed_row_width`, but we never update that width when the host sends escape sequences that move the cursor without writing characters.

## Proposed Next Steps
1. **Simulate full handshake:**
   - Extend tests to emit `WireHostFrame::Grid` + `Snapshot` sequence with rect updates before the prompt row arrives. Confirm the cursor remains at 0 once the prompt text is applied.
2. **Refine committed width tracking:**
   - Reset `logical_width` when we apply non-printing control sequences (e.g., ESC `[K`, `Ctrl+U`), mimicking mosh’s reliance on “damage” packets rather than width heuristics.
   - Consider storing the cursor column reported by the host (if cursor sync is enabled) and trusting it once snapshot completes.
3. **First-line overlay:**
   - Force `prediction_overlay.visible = true` when predictions exist but `srtt` has not yet been sampled. Alternatively, treat the first ACK after handshake as “quick confirmation” for overlay purposes.
4. **Backspace prediction clamp:**
   - Compare predicted column against both committed width and the most recent server cursor hint; if the server moves the cursor left, let the prediction follow instead of clamping.
5. **Audit for mosh parity:**
   - Review mosh’s `Terminal::R`, `Damage::clear()` and predictive backspace logic to see how they coalesce blanking operations. Porting their approach may reduce our heuristics.
6. **Re-run manual QA:**
   - After implementing the above, repeat manual tests: initial prompt, typing `hello world`, spacing, `Backspace`, and `Ctrl+U`.

## Outstanding Issues / Follow-Up Owners
- **Cursor Snapback (First Line):** Pending; needs more precise handling post-snapshot. Owner: current agent.
- **Predictive underline on first line:** Pending; tie overlay visibility to `hasPredictions` even before RTT samples. Owner: current agent / follow-up.
- **Backspace jitter:** Pending; rely on server cursor hints once available.
- **Playwright e2e:** `tests/header-overlap.spec.ts` still blocked by harness configuration.
- **Rust shell quote test:** `tests::shell_quote_handles_spaces_and_quotes` continues to fail due to unrelated CLI change.

## Useful Commands
```bash
# run rust tests (with new predictive coverage)
cargo test -p beach-human

# run web unit tests (ignores Playwright)
npm --prefix apps/beach-web test -- --runInBand --passWithNoTests tests/header-overlap.spec.ts

# enable trace logging for cursor debug
export RUST_LOG=client::render=trace
cargo run -p beach-human -- join <session>
```

## References
- Prior issues: `docs/TERMINAL_CLIENT_ISSUES.md` (predictive echo gaps).
- mosh implementation: `src/frontend/predict.cpp`, `src/terminal/terminal.cc` (for cursor and prediction handling).

---
_Last updated: 2025-03-27._

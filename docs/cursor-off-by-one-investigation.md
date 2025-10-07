# Cursor Off-By-One Investigation

**Status:** OPEN — multiple cursor regressions (missing initial cursor, incorrect scroll position, off-by-one after Enter)
**Date:** October 7, 2025
**Log File:** `/Users/arellidow/development/beach/temp/beach-web.log`

## Problem Description

After implementing predictive cursor advancement in beach-web, cursor positioning has two issues:

1. **FIXED:** Cursor initially appeared in upper-left corner (0, 0) when first connecting
2. **OPEN:** After typing a command and pressing Enter, cursor appears **one column to the left** of where it should be

### Reproduction

```
(base) arellidow@Arels-MacBook-Pro ~ % echo yo yo mf
yo yo mf
(base) arellidow@Arels-MacBook-Pro ~ %_
```

The cursor `_` appears at column 38 when it should be at column 39 (after the `%` and space).

### Observed Behavior from Logs

From `temp/beach-web.log`:

1. **Line 1012:** After Enter is pressed, cursor moves to new row 159:
   ```
   cursor: 'row=159 col=0 seq=1453 visible=true'
   ```

2. **Line 1033:** Then to row 160 (new prompt line):
   ```
   cursor: 'row=160 col=0 seq=1461 visible=true'
   ```

3. **Line 1082:** Server sends final cursor position at column 38:
   ```
   cursor: 'row=160 col=38 seq=1497 visible=true'
   ```

The server position (col=38) appears to be **one column before** the actual input position.

## Latest Findings (October 8, 2025)

1. **Cursor hidden on connect:** Suppressing the first `(0, 0)` cursor frame means we never draw a caret until another authoritative update arrives. The server sticks at `(0, 0)` until the first keypress, so the web client looks cursorless on startup.
2. **Viewport jumps to historic rows:** `followTail` scrolls to the tail of the 160-row snapshot we receive, even when only the most recent 2 rows contain non-empty content. Result: the initial render is scrolled hundreds of rows past the prompt.
3. **Off-by-one still present:** After Enter, the server continues to report `col=38`. The web client now trusts the server frame (no longer clamps to committed width), so the cursor still lands one cell to the left. This points back to the server/emulator generating the wrong column rather than the web clamp.

### Next Checks
- Revisit the initial cursor suppression so we show a caret on connect (maybe only skip rendering the flash but keep visibility when no alternative frame arrives).
- Investigate why the initial snapshot includes 160 rows; confirm whether the emulator is reusing history even for a fresh session or if we need to clamp the viewport before first paint.
- Instrument the Rust emulator (`AlacrittyEmulator::compute_cursor_components`) to log the raw column it emits right after a prompt write, and compare against the prompt string length.


## What We've Fixed

### 1. Initial Cursor Flash at (0, 0) ✅

**Problem:** Cursor briefly appeared in upper-left corner when first connecting.

**Root Cause:** Server sends cursor position (0, 0) in initial snapshot frame. We were displaying it even though it's at the beginning of the prompt line, not a meaningful position.

**Solution:** Added `firstCursorReceived` flag and logic to suppress cursor visibility if the first position received is (0, 0).

**Files Changed:**
- `apps/beach-web/src/terminal/cache.ts:161` - Added `firstCursorReceived` flag
- `apps/beach-web/src/terminal/cache.ts:758-765` - Suppress initial (0, 0) cursor in `applyCursorFrame()`

**Code:**
```typescript
// Suppress initial cursor at (0, 0) to avoid flash in upper-left corner
if (!this.firstCursorReceived && row === 0 && col === 0) {
  this.cursorVisible = false;
  this.firstCursorReceived = true;
} else {
  this.cursorVisible = frame.visible;
  this.firstCursorReceived = true;
}
```

⚠️ **Regression:** Because the server does not send another cursor frame until the user types, suppressing the first `(0, 0)` update leaves the UI without any caret right after connect. We need to refine this logic (e.g., delay suppression or synthesize a visible cursor once the prompt row arrives).

### 2. Predictive Cursor Advancement Implementation ✅

**Problem:** Cursor wasn't advancing immediately when typing - had to wait for server confirmation.

**Solution:** Implemented comprehensive predictive cursor system matching Rust client behavior.

**Files Changed:**
- `apps/beach-web/src/terminal/cache.ts`
  - Lines 65-70: Added `cursorRow` and `cursorCol` to `PendingPredictionEntry`
  - Lines 172, 320: Changed initial `cursorVisible` from `true` to `false`
  - Lines 899-920: Implemented `latestPredictionCursor()` method
  - Lines 922-936: Implemented `updateCursorFromPredictions()` method
  - Lines 1233-1250: Start new predictions from latest prediction's cursor
  - Line 1290: Store cursor position when creating predictions
  - Lines 1352-1355: Always update display cursor to latest prediction

- `apps/beach-web/src/styles.css:63`
  - Added `caret-color: transparent` to hide browser's default caret

**Behavior:** Cursor now advances immediately when typing, before server confirms. Predictions chain correctly - each builds on previous prediction's cursor position.

## Open Issue: Off-By-One After Enter (Server-side Suspect)

### Hypothesis 1: Server Position Accuracy

The server is sending `col=38` but the actual cursor should be at `col=39`. Need to investigate:

1. Is the server calculation wrong?
2. Is the prompt structure causing column mismatch?
3. Are we receiving the correct cursor frame sequence?

### Hypothesis 2: Client Clamping Regressions

We removed the committed-width clamp in `applyCursorFrame()` (`apps/beach-web/src/terminal/cache.ts:744-756`) so the cursor now mirrors whatever column the server reports, up to the terminal width. That puts the spotlight back on the server data: if the prompt row really ends with a space, the emulator should be sending `col=39`, not `38`.

### Hypothesis 3: Prediction Conflict

The cursor frame arrives during/after prediction clearing. Prediction state might be interfering with authoritative cursor positioning.

From logs line 1082, `predictedCursor: null` suggests predictions are cleared when authoritative cursor arrives.

## Investigation Steps for Next Session

1. **Check Prompt Row Content**
   - What is the actual content at row 160?
   - What does `committedRowWidth(160)` return?
   - Is the space after `%` included in committed content?

2. **Compare with Rust Client**
   - Does the Rust client have the same issue?
   - Check `apps/beach/src/client/terminal.rs` for cursor handling after Enter
   - Compare `applyCursorFrame()` logic between web and Rust

3. **Examine Cursor Frame Sequence**
   - Log all cursor frames around Enter keypress
   - Check if intermediate cursor positions are being skipped
   - Verify cursor sequence numbers are monotonic

4. **Instrument server cursor reporting**
   - Add temporary logging to `AlacrittyEmulator::compute_cursor_components()` to dump `(row, col)` alongside the prompt text length
   - Compare against what the web client renders to confirm the off-by-one originates on the server

## Log Analysis Commands

```bash
# View all cursor positions for row 160
grep "row=160 col=" /Users/arellidow/development/beach/temp/beach-web.log

# View snapshot states with context
grep -A 2 -B 2 "snapshot state" /Users/arellidow/development/beach/temp/beach-web.log

# Find Enter key events
grep -i "enter\|return" /Users/arellidow/development/beach/temp/beach-web.log

# Examine cursor frames
grep "applyCursorFrame\|cursor:" /Users/arellidow/development/beach/temp/beach-web.log
```

## Related Files

### Primary Implementation
- `apps/beach-web/src/terminal/cache.ts` - Core terminal state and cursor logic
- `apps/beach-web/src/components/BeachTerminal.tsx` - Cursor rendering

### Reference Implementation
- `apps/beach/src/client/terminal.rs` - Rust client cursor handling

### Logs
- `/Users/arellidow/development/beach/temp/beach-web.log` - Current reproduction log
- Enable trace logging: `window.__BEACH_TRACE = true` in browser console

## Key Code Locations

### Cursor Application (cache.ts)
- `applyCursorFrame()` at line 734 - Applies authoritative cursor from server
- `clampCursor()` at line 1104 - Clamps cursor to valid grid boundaries
- `committedRowWidth()` - Returns width of committed (non-predicted) row content

### Cursor Rendering (BeachTerminal.tsx)
- Lines 1504-1507 - Cursor rendering logic (checks for null before rendering)

### Prediction System (cache.ts)
- `registerPrediction()` at line 1183 - Registers new prediction with cursor position
- `latestPredictionCursor()` at line 899 - Gets latest predicted cursor position
- `updateCursorFromPredictions()` at line 922 - Updates display cursor from predictions

## Context for Next Session

The core predictive cursor system is working correctly - cursor advances immediately when typing. The remaining issue is specifically about the **final cursor position** after pressing Enter and receiving the new prompt from the server.

The cursor ends up at column 38 instead of 39, consistently **one column to the left** of where it should be. This suggests either:
1. The server is calculating/sending the wrong column
2. Our cursor clamping logic is overly restrictive
3. The prompt's trailing space isn't being counted correctly

Start by examining what `committedRowWidth(160)` returns and comparing the actual row content with the cursor position.

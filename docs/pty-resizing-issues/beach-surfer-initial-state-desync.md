# Beach-Surfer Initial State Desynchronization

## Summary

- During an initial join (beach-surfer dev build pointed at `http://localhost:4132`), the terminal view renders only the bottom portion of the host PTY.
- The client cache keeps multiple copies of the HUD/prompt rows (`Ready./Commands/Mode/>`). When follow-tail is enabled, the viewport includes both the latest HUD and stale copies, so the footer appears doubled.
- Because the viewport snaps to `followTail=true` and `viewportTop > 0`, the top of the PTY (rows `0…13`) never paints. The UI shows 48 rows anchored to absolute rows `14…61`, leaving “blank padding” above the HUD.

## Evidence

| Source | Key lines |
| --- | --- |
| `temp/beach-surfer.log:600-940` | `visibleRows tail { viewportTop: 14, viewportHeight: 48, ... rows: [ { absolute: 14, … }, …, { absolute: 61, … } ] }` |
| `temp/beach-surfer.log:760-940` | Duplicate HUD rows: one set at `absolute 44–47` with `seq 361/364`, another at `absolute 58–61` with `seq 155/200/249/251`. |
| Host CLI / Pong TUI | Shows full 62-row viewport; no duplicates in host grid. Alacritty traces confirm `origin=0` and rows `0…61` emitted; no redrawing bug server-side. |

## Impact

- Users joining a session see an empty upper half, even though the host has content there.
- The tail of the viewport repeats stale HUD lines, creating the illusion of duplicated output.
- When the viewer scrolls, the store temporarily disables follow-tail and exposes rows `0…13`, which confirms the host delivered the full buffer; the issue lives in the client cache and viewport math.

## Proposed Fix

1. **Initial viewport anchoring:** On first join, clamp `viewportTop` to the server-advertised origin instead of forcing `followTail=true`. Defer follow-tail until the user scrolls or the cache confirms the newest rows actually differ.
2. **Cache cleanup for stale HUD rows:** When beach-surfer receives full-row snapshots, ensure older HUD rows (same absolute ID) are overwritten or blanked. This prevents the tail view from mixing the latest footer with outdated copies.
3. **Optional guard:** If a viewer reports more visible rows than the host PTY height, keep the viewport capped at the PTY height so the top rows remain visible until the user intentionally scrolls away.

### Implementation Notes

- In `TerminalGridCache.setViewport`, treat the first viewport assignment as authoritative (set row 0 when `knownBaseRow` is `null`). Only re-enable follow-tail once the user scrolls to the bottom or an explicit flag is set.
- Add a helper to mark the newest snapshot rows as “authoritative” so we overwrite any older copies (e.g., when `visibleRows` requests rows 0…n for the first time).
- Update the React layer (`BeachTerminal.tsx`) so `followTail` isn’t forced to `true` immediately; start in “top pinned” mode and toggle only after a user scroll/resize event.

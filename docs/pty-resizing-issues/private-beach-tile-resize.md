# Private Beach Tile Resize Regression

## Summary

- Resizing a `PrivateBeach` terminal tile triggers `WireClientFrame::Resize`, shrinking the host PTY (`cols=104 rows=39`, `rows=45`, `rows=44` in `/tmp/beach-host.log`).
- The tile viewer stays in follow-tail mode when its viewport grows. Because it pads with recent history instead of blanks, the HUD/prompt rows appear twice.

## Evidence

| Source | Excerpt |
| --- | --- |
| `/tmp/beach-host.log` | `processed resize request … cols=104 rows=39`, `… rows=45`, `… rows=44` |
| `temp/private-beach.log:2400‑2710` | `viewportTop: 19`, `viewportHeight: 43`, `followTail: true`, rows `absolute: 19…61`, including two HUD copies (`absolute 40‑43` and `58‑61`). |
| Host terminal after resize | Screen shows only ~32 rows—the host PTY shrank to the latest resize. |

## Proposed Fix

1. **PTy resize opt-in:** Make viewer-driven PTY resizing optional. Default to *not* forwarding resize events. After the user resizes a tile, show an icon/button (“Resize host terminal”) that, when clicked, sends a single `resize` frame matching the tile’s height.
2. **Tail padding:** When the tile viewport exceeds the PTY height, pad the tail with blanks instead of pulling history rows. That keeps the visible area free of duplicate HUD/prompt lines until the host actually scrolls further.

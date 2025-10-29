# Private Beach Terminal Tile Sizing – April 2025 Deep Dive

## 1. Problem Statement
We need tiles that:
- fit within a bounded preview box (≈450×450 px),
- preserve the PTY aspect ratio after host metadata arrives,
- avoid single-row collapse and right-side gutter,
- and never balloon to fill the dashboard.

Today’s pipeline:
1. **BeachTerminal** renders xterm and reports viewport dimensions.
2. **SessionTerminalPreviewClient** wraps it with `transform: scale(...)`.
3. **TileCanvas** (React Grid Layout) manages `w/h/x/y` and computes zoom.

Mismatch between grid footprint, zoom, and PTY metadata makes the preview either too narrow (padding) or huge (tile eats board).

## 2. Journey So Far
| Step | Change | Outcome |
| --- | --- | --- |
| A | Added logging ([tile-layout] tile-zoom, [terminal] viewport-dims) | Confirmed PTY 62×104, zoom clamps to ~0.16, content left-only.
| B | Threaded host rows/cols through preview → TileCanvas | Zoom became stable (~0.25) but gutter persisted (tile still 3×3).
| C | Disabled BeachTerminal line-height measurement when scaled | Fixed single-row collapse but width gutter remained.
| D | Autosized tile to match host pixels | Tile expanded to ~5×12, taking most of dashboard; gutter still there.
| E | Raised default tile to 4×9 | Tile now ~1000 px tall and still padded; zoom height-constrained.
| F | Discussed 128-column approach (not yet implemented) | Current grid still 12 columns/110 px rows, so issues remain.

## 3. What the Logs Show
- `temp/private-beach.log`: repeatedly logs `zoom: 0.56`, `measurements.width ≈ 496`, `height ≈ 990`, `hostRows: 62`. Also `layout-signature [{

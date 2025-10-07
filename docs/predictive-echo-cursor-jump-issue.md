# Predictive Echo Cursor Jump Issue

**Status**: RESOLVED
**Date**: 2025-10-07
**Severity**: High - Affects user experience significantly

## Summary

The cursor jump artifacts during predictive echo were resolved by decoupling display cursor updates from server acknowledgements and preserving predicted overlays until they naturally clear. Specifically, server cursor frames no longer trim predictions when the predicted cursor runs ahead, so the displayed cursor remains anchored at the predicted position. This matches Mosh's behavior where predictions are a purely visual overlay on top of the terminal state.

## Key Changes

- Introduced `predictions_active()` and `update_server_cursor()` helpers to keep the display cursor under prediction control unless no predictions are active (`apps/beach/src/client/terminal.rs:906`, `apps/beach/src/client/terminal.rs:916`).
- Updated wire update handling to always recompute the display cursor via `refresh_prediction_cursor()` instead of mutating it directly (`apps/beach/src/client/terminal.rs:1713`, `apps/beach/src/client/terminal.rs:1766`).
- Adjusted cursor frame handling to avoid trimming predictions while they are active, preventing the server cursor from snapping the display back (`apps/beach/src/client/terminal.rs:1784`).
- Refreshed prediction registration to use the new cursor helpers so forward-typed characters stay visually ahead until the server catches up (`apps/beach/src/client/terminal.rs:3569`).

## Verification

- Local reproduction via SSH session typing tests now shows smooth predictive cursor behavior with no backward jumps.
- `cargo fmt`
- `cargo check`

## Follow-up

- Expand automated regression coverage to capture cursor alignment during predictive echo scenarios.
- Continue studying Mosh's overlay model for other potential UX improvements.


# Visible Driver Investigation – 2025-11-03

## Background
We have been chasing the long-and-skinny terminal tile regression for several days. The working theory has consistently involved two cooperating bugs:

1. **Browser-side host fallback** – whenever the preview emits a bogus measurement (0/1 rows) the BeachTerminal component inside the browser resets its cached PTY height to either the default 24 rows or the 106-row history buffer. Once that happens every viewer sees incorrect geometry.
2. **Hidden driver measurement collapse** – the off-screen BeachTerminal we use for sizing routinely collapses to zero height during StrictMode remounts, so it keeps feeding the bogus values that trigger (1).

To isolate the measurement collapse we introduced a feature flag (`NEXT_PUBLIC_PRIVATE_BEACH_VISIBLE_DRIVER=true`) and switched the dashboard to render a *single visible* BeachTerminal instead of the hidden driver + scaled clone. This should prevent the DOM from ever reporting a 0px height.

## Current Behaviour (with visible driver enabled)
Even with the visible driver path active the tile still stretches vertically and shrinks horizontally. The preview never settles on the server PTY’s 62×106 geometry.

Key observations from the latest run (`temp/private-beach.log`, session `9bf4c223-682c-46ec-a0b3-95d53c4192fa`):

- The flag is active – we see `[terminal][diag] mount … visibleDriver: true` in the log, confirming the visible driver path.
- The host reports the correct PTY (`host-dimensions … rows: 62, cols: 106`) but the viewportState and preview measurements oscillate between smaller values (33, 49, 62, 75). The preview keeps re-scaling to fit those transient numbers, landing at scale ≈ 0.90 and target width ≈ 540px – hence the tall, skinny tile.
- `emit-viewport-state` continues to publish the fluctuating `viewportRows` to all clones even though the PTY rows remain fixed at 62.

Representative log snippet:

```
SessionTerminalPreviewClient.tsx:562 [terminal][diag] host-dimensions {rows: 62, cols: 106, viewportRows: 33, ...}
SessionTerminalPreviewClient.tsx:967 [terminal][trace] viewport-clamped {desiredHeight: 62, snapshotHeight: 49}
SessionTerminalPreviewClient.tsx:903 [terminal][trace] preview-measurements {scale: 0.9033, targetWidth: 549, targetHeight: 720, hostRows: 45, hostCols: 80}
```

So even with the visible driver running, we’re still mixing DOM-derived viewport readings with server PTY sizes, and the oscillation drives the tile to an incorrect aspect ratio.

## Root-Cause Summary
The core issue remains “driver measurement vs. browser host fallback,” but the visible-driver experiment shows an additional nuance:

- The BeachTerminal *still* writes whatever viewport height it measures (33, 49, 75) into the shared store, even though the PTY rows are fixed at 62. Because we no longer distinguish between the driver and the clone, the visible terminal directly inherits that store viewport height and rescales to fit it. This is effectively the same bug path as before, just without the hidden-driver indirection.

In other words, the measurement pipeline is still authoritative even though it should only supply pixels. We need to keep the PTY rows from the server untouched and make viewport updates purely cosmetic.

## Suggested Next Steps
1. **Separate PTY height vs. viewport height** – ensure the store and preview keep the PTY row count from the server metadata (`frame.viewportRows`) and only use DOM measurements to derive pixel scaling. The beach host should not rewrite `ptyViewportRowsRef` when the DOM reports a smaller viewport.
2. **Debounce DOM measurements** – even with a visible terminal, small oscillations (e.g. 62 → 49 → 75) show up in the log. Wait for two consecutive samples or a small time window before writing a new viewport height to the store.
3. **Audit `emit-viewport-state` consumers** – confirm the viewer never mistakes `viewportRows` for “host rows.” If a consumer still uses the viewport height to infer PTY rows, refactor it to use the explicit `hostViewportRows` field instead.

With those changes we expect the tile to scale exactly once (when the first stable DOM measurement lands) and to hold the real PTY aspect ratio afterwards.

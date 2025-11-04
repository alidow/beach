# Beach Terminal Integration Guide

## Overview

The rewrite tiles now mount the hardened `BeachTerminal` component from `apps/beach-surfer`. Sizing is delegated to the new `TerminalSizingStrategy` contract so tiles can opt into layout policies without inheriting legacy DOM measurements.

## Key Pieces

- **Sizing strategy** — Implemented at `apps/private-beach-rewrite/src/components/rewriteTerminalSizing.ts`. The strategy divides the tile height by the terminal line height and reports the viewport rows while leaving width management to the parent layout.
- **Legacy default** — `apps/private-beach/src/components/terminalSizing.ts` exports the legacy strategy (`default` export) used implicitly by the existing dashboard. No caller changes required there.
- **Optional UI toggles** — `BeachTerminal` now accepts:
  - `enablePredictiveEcho` (default `true`)
  - `enableKeyboardShortcuts` (default `true`)
  - `showJoinOverlay` (default `true`)
  These allow consumers to disable predictive UX, keyboard affordances, or the host approval overlay.

## Wiring the Tile

1. Import the terminal and rewrite sizing strategy:
   ```ts
   import { BeachTerminal } from '../../../beach-surfer/src/components/BeachTerminal';
   import { rewriteTerminalSizingStrategy } from './rewriteTerminalSizing';
   ```
2. Render the terminal inside the tile container:
   ```tsx
   <BeachTerminal
     store={viewer.store ?? undefined}
     transport={viewer.transport ?? undefined}
     autoConnect={false}
     showTopBar={false}
     showStatusBar={false}
     hideIdlePlaceholder
     sizingStrategy={rewriteTerminalSizingStrategy}
     sessionId={sessionId ?? undefined}
     showJoinOverlay={false}
     enablePredictiveEcho={false}
   />
   ```
   - `autoConnect={false}` keeps control with the existing viewer connection flow.
   - Overlays are trimmed for the rewrite (`showJoinOverlay={false}`) while keyboard input remains enabled.
3. Preserve the existing status overlays by layering the previous `session-viewer__overlay` divs above the terminal when desired.
4. Styling adjustments live in `globals.css`. The container now fills the tile (`.session-viewer__terminal { width: 100%; height: 100%; }`) while the terminal’s own chrome provides the visual treatment.

## Legacy Dashboard Compatibility

No dashboard code changes are required: `BeachTerminal` defaults to the legacy sizing strategy and keeps predictive echo, keyboard shortcuts, and overlays enabled. Existing telemetry and predictive logging still fire because the hook extraction preserves the original side-effects.

## Next Steps

- Iterate on `rewriteTerminalSizingStrategy` as new tile constraints emerge (e.g., clamped heights or viewport padding).
- Decide which optional affordances (predictive echo, join overlay) to re-enable once the rewrite UX settles.
- Extend Playwright coverage to assert that the rewrite tile renders the Beach terminal and responds to keyboard input.

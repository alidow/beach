import type { CSSProperties } from 'react';
import type {
  TerminalSizingStrategy,
  TerminalSizingHostMeta,
  TerminalViewportProposal,
  TerminalScrollPolicy,
} from '../../../private-beach/src/components/terminalSizing';

export class RewriteTerminalSizingStrategy implements TerminalSizingStrategy {
  nextViewport(tileRect: DOMRectReadOnly, hostMeta: TerminalSizingHostMeta): TerminalViewportProposal {
    const rowHeight = Math.max(1, Math.floor(hostMeta.lineHeightPx));
    if (rowHeight <= 0) {
      const fallback =
        hostMeta.preferredViewportRows ??
        hostMeta.lastViewportRows ??
        hostMeta.defaultViewportRows;
      return { viewportRows: Math.max(1, fallback ?? hostMeta.defaultViewportRows) };
    }

    const height = Number.isFinite(tileRect.height) ? Math.max(0, tileRect.height) : 0;
    if (height <= 0) {
      const fallback =
        hostMeta.preferredViewportRows ??
        hostMeta.lastViewportRows ??
        hostMeta.defaultViewportRows;
      return { viewportRows: Math.max(1, fallback ?? hostMeta.defaultViewportRows) };
    }

    const measuredRows = Math.max(1, Math.floor(height / rowHeight));
    const viewportRows = Math.max(1, Math.min(measuredRows, hostMeta.maxViewportRows));
    return {
      viewportRows,
      measuredRows,
    };
  }

  containerStyle(
    _tileRect: DOMRectReadOnly,
    hostMeta: TerminalSizingHostMeta,
    viewportRows: number,
  ): CSSProperties | undefined {
    const rowHeight = Math.max(1, Math.floor(hostMeta.lineHeightPx));
    const heightPx = Math.max(1, Math.ceil(viewportRows * rowHeight));
    return {
      '--beach-terminal-max-height': `${heightPx}px`,
    };
  }

  scrollPolicy(): TerminalScrollPolicy {
    return 'follow-tail';
  }
}

export const rewriteTerminalSizingStrategy = new RewriteTerminalSizingStrategy();

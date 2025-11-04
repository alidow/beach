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
    const preferred =
      typeof hostMeta.preferredViewportRows === 'number' && hostMeta.preferredViewportRows > 0
        ? hostMeta.preferredViewportRows
        : null;
    const targetRows = preferred ?? measuredRows;
    const viewportRows = Math.max(
      hostMeta.minViewportRows,
      Math.min(targetRows, hostMeta.maxViewportRows),
    );
    if (typeof window !== 'undefined' && window.__BEACH_TRACE) {
      try {
        console.info('[rewrite-sizing] rows', {
          measuredRows,
          preferred,
          targetRows,
          min: hostMeta.minViewportRows,
          max: hostMeta.maxViewportRows,
          result: viewportRows,
        });
      } catch {
        // ignore logging errors
      }
    }
    return {
      viewportRows,
      measuredRows,
      fallbackRows: preferred ?? undefined,
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

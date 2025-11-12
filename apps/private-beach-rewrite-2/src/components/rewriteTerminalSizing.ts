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
    const clampRows = (rows: number | null | undefined): number => {
      const numeric = Number.isFinite(rows) && rows != null ? Math.round(rows as number) : 0;
      const safe = numeric > 0 ? numeric : hostMeta.defaultViewportRows;
      return Math.max(hostMeta.minViewportRows, Math.min(safe, hostMeta.maxViewportRows));
    };

    const resolveFallback = (): number => {
      if (hostMeta.forcedViewportRows && hostMeta.forcedViewportRows > 0) {
        return hostMeta.forcedViewportRows;
      }
      if (hostMeta.lastViewportRows && hostMeta.lastViewportRows > 0) {
        return hostMeta.lastViewportRows;
      }
      if (hostMeta.preferredViewportRows && hostMeta.preferredViewportRows > 0) {
        return hostMeta.preferredViewportRows;
      }
      return hostMeta.defaultViewportRows;
    };

    if (hostMeta.disableViewportMeasurements) {
      const fallbackRows = resolveFallback();
      return {
        viewportRows: clampRows(fallbackRows),
        fallbackRows,
      };
    }

    if (rowHeight <= 0) {
      const fallbackRows = resolveFallback();
      return { viewportRows: clampRows(fallbackRows), fallbackRows };
    }

    const height = Number.isFinite(tileRect.height) ? Math.max(0, tileRect.height) : 0;
    if (height <= 0) {
      const fallbackRows = resolveFallback();
      return { viewportRows: clampRows(fallbackRows), fallbackRows };
    }

    const measuredRows = Math.max(1, Math.floor(height / rowHeight));
    const rawPreferred =
      typeof hostMeta.preferredViewportRows === 'number' && hostMeta.preferredViewportRows > 0
        ? hostMeta.preferredViewportRows
        : null;
    const preferred =
      rawPreferred !== null ? clampRows(rawPreferred) : null;
    const overshootAllowance = Math.max(2, Math.ceil(measuredRows * 0.2));
    const preferredTapered =
      preferred !== null ? Math.min(preferred, measuredRows + overshootAllowance) : null;
    const targetRows = preferredTapered ?? measuredRows;
    const viewportRows = clampRows(targetRows);
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
    const fallbackRows = rawPreferred ?? undefined;
    return {
      viewportRows,
      measuredRows,
      fallbackRows,
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
    } as CSSProperties;
  }

  scrollPolicy(): TerminalScrollPolicy {
    return 'follow-tail';
  }
}

export const rewriteTerminalSizingStrategy = new RewriteTerminalSizingStrategy();

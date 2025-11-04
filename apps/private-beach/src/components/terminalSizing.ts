import type { CSSProperties } from 'react';

export type TerminalScrollPolicy = 'follow-tail' | 'manual';

export interface TerminalSizingHostMeta {
  lineHeightPx: number;
  minViewportRows: number;
  maxViewportRows: number;
  lastViewportRows: number | null;
  disableViewportMeasurements: boolean;
  forcedViewportRows: number | null;
  preferredViewportRows: number | null;
  windowInnerHeightPx: number | null;
  defaultViewportRows: number;
}

export interface TerminalViewportProposal {
  viewportRows: number | null;
  measuredRows?: number;
  fallbackRows?: number;
}

export interface TerminalSizingStrategy {
  nextViewport(tileRect: DOMRectReadOnly, hostMeta: TerminalSizingHostMeta): TerminalViewportProposal;
  containerStyle(
    tileRect: DOMRectReadOnly,
    hostMeta: TerminalSizingHostMeta,
    viewportRows: number,
  ): CSSProperties | undefined;
  scrollPolicy(): TerminalScrollPolicy;
}

export class LegacyTerminalSizingStrategy implements TerminalSizingStrategy {
  private clampRows(rows: number, max: number): number {
    const rounded = Number.isFinite(rows) ? Math.round(rows) : 0;
    if (!Number.isFinite(rounded) || rounded <= 0) {
      return 1;
    }
    return Math.max(1, Math.min(rounded, max));
  }

  nextViewport(tileRect: DOMRectReadOnly, hostMeta: TerminalSizingHostMeta): TerminalViewportProposal {
    const rowHeight = Math.max(1, Math.floor(hostMeta.lineHeightPx));
    const proposal: TerminalViewportProposal = { viewportRows: null };

    if (hostMeta.disableViewportMeasurements) {
      const desired = this.resolvePreferredRows(hostMeta);
      const candidate = this.stabilizeRows(desired, hostMeta);
      proposal.viewportRows = this.clampRows(candidate, hostMeta.maxViewportRows);
      return proposal;
    }

    const viewportHeight = Number.isFinite(tileRect.height) ? Math.max(0, tileRect.height) : 0;
    if (viewportHeight <= 0) {
      const fallback = this.resolvePreferredRows(hostMeta);
      proposal.viewportRows = this.clampRows(fallback, hostMeta.maxViewportRows);
      return proposal;
    }

    const measuredRows = Math.max(1, Math.floor(viewportHeight / rowHeight));
    proposal.measuredRows = measuredRows;

    const windowRows = hostMeta.windowInnerHeightPx != null
      ? Math.max(1, Math.floor(hostMeta.windowInnerHeightPx / rowHeight))
      : hostMeta.maxViewportRows;
    const fallbackRows = Math.max(1, Math.min(windowRows, hostMeta.maxViewportRows));
    proposal.fallbackRows = fallbackRows;

    const stabilized = this.stabilizeRows(measuredRows, hostMeta, fallbackRows);
    proposal.viewportRows = this.clampRows(stabilized, hostMeta.maxViewportRows);
    return proposal;
  }

  containerStyle(
    _tileRect: DOMRectReadOnly,
    hostMeta: TerminalSizingHostMeta,
    viewportRows: number,
  ): CSSProperties | undefined {
    const rowHeight = Math.max(1, Math.floor(hostMeta.lineHeightPx));

    if (hostMeta.disableViewportMeasurements) {
      // Match legacy behaviour when viewport measurements are disabled.
      const heightPx = Math.max(1, Math.ceil(viewportRows * rowHeight) + 2);
      return {
        maxHeight: `${heightPx}px`,
        height: `${heightPx}px`,
        overflowY: 'hidden',
        '--beach-terminal-max-height': `${heightPx}px`,
      };
    }

    const maxHeightPx = Math.max(1, Math.ceil(viewportRows * rowHeight));
    return {
      maxHeight: `${maxHeightPx}px`,
      '--beach-terminal-max-height': `${maxHeightPx}px`,
    };
  }

  scrollPolicy(): TerminalScrollPolicy {
    return 'follow-tail';
  }

  private resolvePreferredRows(hostMeta: TerminalSizingHostMeta): number {
    if (hostMeta.forcedViewportRows && hostMeta.forcedViewportRows > 0) {
      return hostMeta.forcedViewportRows;
    }
    if (hostMeta.preferredViewportRows && hostMeta.preferredViewportRows > 0) {
      return hostMeta.preferredViewportRows;
    }
    if (hostMeta.lastViewportRows && hostMeta.lastViewportRows > 0) {
      return hostMeta.lastViewportRows;
    }
    return Math.max(1, hostMeta.defaultViewportRows);
  }

  private stabilizeRows(
    candidate: number,
    hostMeta: TerminalSizingHostMeta,
    fallbackRows?: number,
  ): number {
    let next = candidate;
    if (fallbackRows != null && fallbackRows > 0) {
      next = Math.min(next, fallbackRows);
    }
    const previous = hostMeta.lastViewportRows;
    if (
      previous != null &&
      previous >= hostMeta.minViewportRows &&
      next <= Math.max(1, Math.floor(hostMeta.minViewportRows / 2))
    ) {
      return previous;
    }
    return next;
  }
}

export function createLegacyTerminalSizingStrategy(): TerminalSizingStrategy {
  return new LegacyTerminalSizingStrategy();
}

export const legacyTerminalSizingStrategy = createLegacyTerminalSizingStrategy();

export default legacyTerminalSizingStrategy;

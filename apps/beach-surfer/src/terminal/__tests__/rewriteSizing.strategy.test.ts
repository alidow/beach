import { describe, it, expect } from 'vitest';
import { RewriteTerminalSizingStrategy } from '../../../../private-beach-rewrite-2/src/components/rewriteTerminalSizing';
import type { TerminalSizingHostMeta } from '../../../../private-beach/src/components/terminalSizing';

const makeMeta = (overrides: Partial<TerminalSizingHostMeta> = {}): TerminalSizingHostMeta => ({
  lineHeightPx: 20,
  minViewportRows: 6,
  maxViewportRows: 512,
  lastViewportRows: 24,
  disableViewportMeasurements: false,
  forcedViewportRows: null,
  preferredViewportRows: null,
  windowInnerHeightPx: 800,
  defaultViewportRows: 24,
  ...overrides,
});

const mockRect = (width: number, height: number): DOMRectReadOnly => ({
  width,
  height,
  x: 0,
  y: 0,
  top: 0,
  left: 0,
  right: width,
  bottom: height,
  toJSON() {
    return {};
  },
});

describe('RewriteTerminalSizingStrategy', () => {
  const strategy = new RewriteTerminalSizingStrategy();

  it('captures regression where preferred rows override measured rows after resize', () => {
    const meta = makeMeta({ preferredViewportRows: 106 });
    const rect = mockRect(964, 218); // ~10 visible rows with 20px line height

    const proposal = strategy.nextViewport(rect, meta);

    // Desired future behaviour: clamp to the measured rows (≈10) while data streams in.
    // Current implementation returns 106, leading to a placeholder-only tail viewport.
    expect(proposal.viewportRows).toBeLessThanOrEqual(12);
  });

  it('falls back to last measured rows when measurements are disabled for layout throttling', () => {
    const meta = makeMeta({
      disableViewportMeasurements: true,
      preferredViewportRows: 120,
      lastViewportRows: 18,
    });
    const rect = mockRect(900, 0);

    const proposal = strategy.nextViewport(rect, meta);

    expect(proposal.viewportRows).toBe(18);
    expect(proposal.fallbackRows).toBe(18);
  });

  it('uses preferred rows when no previous measurement is available', () => {
    const meta = makeMeta({
      disableViewportMeasurements: true,
      preferredViewportRows: 120,
      lastViewportRows: 0,
    });
    const rect = mockRect(900, 0);

    const proposal = strategy.nextViewport(rect, meta);

    expect(proposal.viewportRows).toBe(120);
    expect(proposal.fallbackRows).toBe(120);
  });
});

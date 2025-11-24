import { describe, it, expect, afterEach, vi } from 'vitest';
import { render, cleanup } from '@testing-library/react';
import type { TerminalViewportState } from '../../../../beach-surfer/src/components/BeachTerminal';
import { SessionViewer } from '../SessionViewer';

const beachTerminalProps: { current: Record<string, unknown> | null } = { current: null };

vi.mock('../../../../beach-surfer/src/components/BeachTerminal', () => ({
  BeachTerminal: (props: Record<string, unknown>) => {
    beachTerminalProps.current = props;
    return null;
  },
}));

describe('SessionViewer viewport metrics', () => {
  afterEach(() => {
    beachTerminalProps.current = null;
    cleanup();
  });

  const baseViewer = {
    store: null,
    transport: null,
    connecting: false,
    error: null,
    status: 'connected' as const,
    secureSummary: null,
    latencyMs: null,
    transportVersion: 0,
  };

  const sampleState = (): TerminalViewportState => ({
    viewportRows: 40,
    viewportCols: 120,
    hostViewportRows: 40,
    hostCols: 120,
    canSendResize: false,
    viewOnly: false,
    followTailDesired: true,
    followTailPhase: 'follow_tail',
    atTail: true,
    remainingTailPixels: 0,
    tailPaddingRows: 0,
    pixelsPerRow: 18,
    pixelsPerCol: 9,
  });

  it('reports normalized metrics from BeachTerminal updates', () => {
    const onViewportMetrics = vi.fn();
    render(
      <SessionViewer
        viewer={baseViewer}
        tileId="tile-v"
        sessionId="sess-1"
        onViewportMetrics={onViewportMetrics}
      />,
    );
    onViewportMetrics.mockClear();

    expect(beachTerminalProps.current).not.toBeNull();
    const props = beachTerminalProps.current as { onViewportStateChange?: (state: TerminalViewportState) => void };
    props.onViewportStateChange?.(sampleState());
    expect(onViewportMetrics).toHaveBeenCalledWith({
      tileId: 'tile-v',
      hostRows: 40,
      hostCols: 120,
      viewportRows: 40,
      viewportCols: 120,
      pixelsPerRow: 18,
      pixelsPerCol: 9,
      hostWidthPx: null,
      hostHeightPx: null,
      cellWidthPx: 9,
      cellHeightPx: 18,
      quantizedCellWidthPx: null,
      quantizedCellHeightPx: null,
    });
  });

  it('deduplicates repeated viewport payloads', () => {
    const onViewportMetrics = vi.fn();
    render(
      <SessionViewer
        viewer={baseViewer}
        tileId="tile-v"
        sessionId="sess-1"
        onViewportMetrics={onViewportMetrics}
      />,
    );
    onViewportMetrics.mockClear();
    const props = beachTerminalProps.current as { onViewportStateChange?: (state: TerminalViewportState) => void };
    const state = sampleState();
    props.onViewportStateChange?.(state);
    props.onViewportStateChange?.(state);
    expect(onViewportMetrics).toHaveBeenCalledTimes(1);
  });

  it('emits null metrics when session changes', () => {
    const onViewportMetrics = vi.fn();
    const { rerender } = render(
      <SessionViewer
        viewer={baseViewer}
        tileId="tile-v"
        sessionId="sess-1"
        onViewportMetrics={onViewportMetrics}
      />,
    );
    onViewportMetrics.mockClear();
    rerender(
      <SessionViewer
        viewer={baseViewer}
        tileId="tile-v"
        sessionId="sess-2"
        onViewportMetrics={onViewportMetrics}
      />,
    );
    expect(onViewportMetrics).toHaveBeenCalledWith(null);
  });
});

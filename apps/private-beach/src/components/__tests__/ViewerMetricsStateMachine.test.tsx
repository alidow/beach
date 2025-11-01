import { act, render, screen } from '@testing-library/react';
import React, { useEffect, useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import type { TerminalViewerState } from '../../hooks/terminalViewerTypes';

function ViewerStatusBanner({ viewer }: { viewer: TerminalViewerState }) {
  let latencyLabel = 'Latency —';
  if (viewer.latencyMs != null) {
    latencyLabel = viewer.latencyMs >= 1000 ? `Latency ${(viewer.latencyMs / 1000).toFixed(1)}s` : `Latency ${Math.round(viewer.latencyMs)}ms`;
  }
  return (
    <div>
      <span data-testid="status">{viewer.status}</span>
      <span data-testid="latency">{latencyLabel}</span>
      <span data-testid="error">{viewer.error ?? 'none'}</span>
    </div>
  );
}

function ViewerStateHarness({ sequence }: { sequence: Array<{ delay: number; state: TerminalViewerState }> }) {
  const [index, setIndex] = useState(0);
  const [viewer, setViewer] = useState(sequence[0]?.state);

  useEffect(() => {
    if (index >= sequence.length - 1) {
      return;
    }
    const timer = setTimeout(() => {
      const nextIndex = index + 1;
      setIndex(nextIndex);
      setViewer(sequence[nextIndex]?.state);
    }, sequence[index + 1]?.delay ?? 0);
    return () => clearTimeout(timer);
  }, [index, sequence]);

  return <ViewerStatusBanner viewer={viewer} />;
}

describe('viewerMetrics / ViewerStatusBanner', () => {
  it('advances through connecting, connected, and error states with fake timers', () => {
    vi.useFakeTimers();
    try {
      const sequence: Array<{ delay: number; state: TerminalViewerState }> = [
        {
          delay: 0,
          state: {
            store: null,
            transport: null,
            connecting: true,
            error: null,
            status: 'connecting',
            secureSummary: null,
            latencyMs: null,
          },
        },
        {
          delay: 200,
          state: {
            store: null,
            transport: null,
            connecting: false,
            error: null,
            status: 'connected',
            secureSummary: null,
            latencyMs: 64,
          },
        },
        {
          delay: 150,
          state: {
            store: null,
            transport: null,
            connecting: false,
            error: 'keepalive failure detected',
            status: 'error',
            secureSummary: null,
            latencyMs: null,
          },
        },
      ];

      render(<ViewerStateHarness sequence={sequence} />);

      expect(screen.getByTestId('status').textContent).toBe('connecting');
      expect(screen.getByTestId('latency').textContent).toBe('Latency —');

      act(() => {
        vi.advanceTimersByTime(200);
      });
      expect(screen.getByTestId('status').textContent).toBe('connected');
      expect(screen.getByTestId('latency').textContent).toBe('Latency 64ms');

      act(() => {
        vi.advanceTimersByTime(150);
      });
      expect(screen.getByTestId('status').textContent).toBe('error');
      expect(screen.getByTestId('error').textContent).toBe('keepalive failure detected');
    } finally {
      vi.useRealTimers();
    }
  });
});

import React from 'react';
import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { render, waitFor, cleanup, act, fireEvent, screen } from '@testing-library/react';
import type { BeachTerminal as BeachTerminalComponent, TerminalViewportState } from '../BeachTerminal';
import { createTerminalStore } from '../../terminal/useTerminalState';

class StubResizeObserver {
  observe() {}
  disconnect() {}
}

let originalResizeObserver: typeof ResizeObserver | undefined;
let originalRaf: typeof requestAnimationFrame | undefined;
let originalCaf: typeof cancelAnimationFrame | undefined;
let originalFetch: typeof fetch | undefined;
let originalWindowFetch: typeof fetch | undefined;
let BeachTerminal: typeof BeachTerminalComponent;

describe('BeachTerminal tail intent state machine', () => {
  beforeAll(async () => {
    originalResizeObserver = (global as any).ResizeObserver;
    originalRaf = (global as any).requestAnimationFrame;
    originalCaf = (global as any).cancelAnimationFrame;
    originalFetch = (global as any).fetch;
    originalWindowFetch = typeof window !== 'undefined' ? (window as any).fetch : undefined;
    (global as any).ResizeObserver = StubResizeObserver;
    const raf = (cb: FrameRequestCallback) => setTimeout(() => cb(performance.now()), 0);
    const caf = (id: number) => clearTimeout(id);
    (global as any).requestAnimationFrame = raf;
    (global as any).cancelAnimationFrame = caf;
    (global as any).fetch = async (input: RequestInfo | URL) => {
      const url = typeof input === 'string' ? input : input instanceof URL ? input.href : '';
      if (url.includes('argon2.wasm')) {
        return new Response(new Uint8Array([0]), { status: 200 });
      }
      if (typeof originalFetch === 'function') {
        return originalFetch(input as any);
      }
      return new Response('{}', { status: 200 });
    };
    if (typeof window !== 'undefined') {
      (window as any).fetch = (global as any).fetch;
    }
    const mod = await import('../BeachTerminal');
    BeachTerminal = mod.BeachTerminal;
  });

  afterAll(() => {
    cleanup();
    (global as any).ResizeObserver = originalResizeObserver;
    (global as any).requestAnimationFrame = originalRaf;
    (global as any).cancelAnimationFrame = originalCaf;
    (global as any).fetch = originalFetch;
    if (typeof window !== 'undefined') {
      (window as any).fetch = originalWindowFetch;
    }
  });

  it('keeps follow-tail intent while tail padding is outstanding', async () => {
    const store = createTerminalStore();
    const viewportStates: TerminalViewportState[] = [];
    render(
      <BeachTerminal
        store={store}
        autoConnect={false}
        disableViewportMeasurements
        hideIdlePlaceholder
        showTopBar={false}
        showStatusBar={false}
        onViewportStateChange={(state) => {
          viewportStates.push(state);
        }}
      />,
    );

    await waitFor(() => {
      expect(viewportStates.length).toBeGreaterThan(0);
    });

    act(() => {
      store.setBaseRow(0);
      store.setGridSize(200, 80);
      const updates = [];
      for (let row = 0; row < 120; row += 1) {
        updates.push({
          type: 'row' as const,
          row,
          seq: row + 1,
          cells: Array.from({ length: 10 }, () => ({ char: 'x', styleId: 0, seq: row + 1 })),
        });
      }
      store.applyUpdates(updates, { authoritative: true });
      store.setViewport(96, 24);
      store.setFollowTail(true);
    });

    await waitFor(() => {
      const latest = viewportStates[viewportStates.length - 1];
      expect(latest?.followTailDesired).toBe(true);
      expect(latest?.followTailPhase === 'follow_tail' || latest?.followTailPhase === 'hydrating').toBeTruthy();
    });

    act(() => {
      store.markPendingRange(120, 134);
    });

    await waitFor(() => {
      const latest = viewportStates[viewportStates.length - 1];
      expect(latest?.followTailDesired).toBe(true);
      expect(['hydrating', 'catching_up', 'follow_tail']).toContain(latest?.followTailPhase);
      expect(latest?.tailPaddingRows ?? 0).toBeGreaterThan(0);
    });
  });

});

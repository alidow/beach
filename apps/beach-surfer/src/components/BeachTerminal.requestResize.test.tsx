import React from 'react';
import { describe, expect, it, beforeAll, afterAll, vi } from 'vitest';
import { render, waitFor, cleanup } from '@testing-library/react';
import type { BeachTerminal as BeachTerminalComponent, TerminalViewportState } from './BeachTerminal';

class StubResizeObserver {
  observe() {}
  disconnect() {}
}

class MockTransport extends EventTarget {
  public sent: Array<{ type: string; rows: number; cols: number }> = [];

  send(payload: { type: string; rows: number; cols: number }) {
    this.sent.push(payload);
  }
}

let originalResizeObserver: typeof ResizeObserver | undefined;
let originalRaf: typeof requestAnimationFrame | undefined;
let originalCaf: typeof cancelAnimationFrame | undefined;
let originalFetch: typeof fetch | undefined;
let originalWindowFetch: typeof fetch | undefined;
let BeachTerminal: typeof BeachTerminalComponent;

describe('BeachTerminal requestHostResize', () => {
  beforeAll(async () => {
    originalResizeObserver = (global as any).ResizeObserver;
    originalRaf = (global as any).requestAnimationFrame;
    originalCaf = (global as any).cancelAnimationFrame;
    originalFetch = (global as any).fetch;
    originalWindowFetch = typeof window !== 'undefined' ? (window as any).fetch : undefined;
    (global as any).ResizeObserver = StubResizeObserver;
    // Provide a soft RAF polyfill so BeachTerminal effects can schedule without tight loops.
    const raf = (cb: FrameRequestCallback) => setTimeout(() => cb(performance.now()), 0);
    const caf = (id: number) => clearTimeout(id);
    (global as any).requestAnimationFrame = raf;
    (global as any).cancelAnimationFrame = caf;
    (global as any).fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = typeof input === 'string' ? input : input instanceof URL ? input.href : '';
      if (url.includes('argon2.wasm')) {
        return new Response(new Uint8Array([0]), { status: 200 });
      }
      if (typeof originalFetch === 'function') {
        return originalFetch(input as any);
      }
      throw new Error('Unhandled fetch request in test');
    });
    if (typeof window !== 'undefined') {
      (window as any).fetch = (global as any).fetch;
    }
    const mod = await import('./BeachTerminal');
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

  it('clamps rows to a minimum of 2 and falls back to host cols', async () => {
    const transport = new MockTransport();
    let viewportState: TerminalViewportState | null = null;
    render(
      <BeachTerminal
        transport={transport as any}
        autoConnect={false}
        disableViewportMeasurements
        hideIdlePlaceholder
        onViewportStateChange={(state) => {
          viewportState = state;
        }}
      />,
    );

    await waitFor(() => {
      expect(viewportState).not.toBeNull();
    });

    expect(viewportState?.viewOnly).toBe(false);
    expect(viewportState?.requestHostResize).toBeDefined();
    viewportState?.requestHostResize?.({ rows: 1 });
    expect(transport.sent.pop()).toEqual({ type: 'resize', rows: 2, cols: 80 });
  });

  it('caps rows at 512 and respects explicit cols input', async () => {
    const transport = new MockTransport();
    let viewportState: TerminalViewportState | null = null;
    render(
      <BeachTerminal
        transport={transport as any}
        autoConnect={false}
        disableViewportMeasurements
        hideIdlePlaceholder
        onViewportStateChange={(state) => {
          viewportState = state;
        }}
      />,
    );

    await waitFor(() => {
      expect(viewportState).not.toBeNull();
    });

    expect(viewportState?.viewOnly).toBe(false);
    expect(viewportState?.requestHostResize).toBeDefined();
    viewportState?.requestHostResize?.({ rows: 999, cols: 12 });
    expect(transport.sent.pop()).toEqual({ type: 'resize', rows: 512, cols: 12 });
  });

  it('suppresses resize helpers and frames in view-only mode', async () => {
    const transport = new MockTransport();
    let viewportState: TerminalViewportState | null = null;
    render(
      <BeachTerminal
        transport={transport as any}
        autoConnect={false}
        disableViewportMeasurements
        hideIdlePlaceholder
        viewOnly
        onViewportStateChange={(state) => {
          viewportState = state;
        }}
      />,
    );

    await waitFor(() => {
      expect(viewportState).not.toBeNull();
    });

    expect(viewportState?.viewOnly).toBe(true);
    expect(viewportState?.canSendResize).toBe(false);
    expect(viewportState?.sendHostResize).toBeUndefined();
    expect(viewportState?.requestHostResize).toBeUndefined();
    expect(transport.sent).toHaveLength(0);
  });
});

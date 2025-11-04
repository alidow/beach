import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Mock } from 'vitest';
import type { TerminalViewerState } from '../../hooks/terminalViewerTypes';
import { getViewerCounters, resetViewerCounters } from '../metricsRegistry';
vi.mock('../../lib/telemetry', () => ({
  emitTelemetry: vi.fn(),
}));
import { emitTelemetry } from '../../lib/telemetry';

let ViewerConnectionService: typeof import('../viewerConnectionService').ViewerConnectionService;
const mockedTelemetry = emitTelemetry as unknown as Mock;

function makeViewerState(
  status: TerminalViewerState['status'],
  overrides: Partial<TerminalViewerState & { transportVersion?: number }> = {},
): TerminalViewerState & { transportVersion?: number } {
  return {
    store: null,
    transport: null,
    transportVersion: 0,
    connecting: status === 'connecting' || status === 'reconnecting',
    error: null,
    status,
    secureSummary: null,
    latencyMs: null,
    ...overrides,
  };
}

describe('viewerMetrics / viewerConnectionService', () => {
  let service: InstanceType<typeof ViewerConnectionService>;

  beforeAll(async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => new Response('', { status: 200, statusText: 'OK' })),
    );
    vi.useFakeTimers();
    ({ ViewerConnectionService } = await import('../viewerConnectionService'));
  });

  beforeEach(() => {
    service = new ViewerConnectionService();
    service.resetMetrics();
    resetViewerCounters();
    mockedTelemetry.mockClear();
  });

  it('captures connects, reconnects, and disconnects via debug emit', () => {
    const subscriber = vi.fn();
    const disconnect = service.connectTile(
      'tile-1',
      { sessionId: '', privateBeachId: null, managerUrl: '', authToken: '' },
      subscriber,
    );

    service.debugEmit('tile-1', makeViewerState('connecting', { connecting: true }));
    service.debugEmit('tile-1', makeViewerState('connected', { connecting: false }));
    service.debugEmit('tile-1', makeViewerState('reconnecting', { connecting: true }));
    service.debugEmit('tile-1', makeViewerState('connected', { connecting: false }));

    const metricsSnapshot = service.getTileMetrics('tile-1');
    expect(metricsSnapshot.started).toBe(2);
    expect(metricsSnapshot.completed).toBe(2);
    expect(metricsSnapshot.retries).toBe(1);
    expect(metricsSnapshot.disposed).toBe(0);

    disconnect();

    const globalCounters = getViewerCounters('tile-1');
    expect(globalCounters?.started).toBe(metricsSnapshot.started);
    expect(globalCounters?.completed).toBe(metricsSnapshot.completed);
    expect(globalCounters?.retries).toBe(metricsSnapshot.retries);
    expect(globalCounters?.disposed).toBe(1);
    expect(subscriber).toHaveBeenCalledTimes(5);
  });

  it('increments failures for keepalive and idle warnings', () => {
    service.connectTile('tile-keepalive', { sessionId: '', privateBeachId: null, managerUrl: '', authToken: '' }, vi.fn());

    service.debugEmit('tile-keepalive', makeViewerState('error', { connecting: false, error: 'keepalive failure' }));
    service.debugEmit('tile-keepalive', makeViewerState('reconnecting', { connecting: true }));
    service.debugEmit('tile-keepalive', makeViewerState('connected', { connecting: false }));
    service.debugEmit(
      'tile-keepalive',
      makeViewerState('error', { connecting: false, error: 'idle warning: host silent' }),
    );

    const metricsSnapshot = service.getTileMetrics('tile-keepalive');
    expect(metricsSnapshot.failures).toBe(2);
  });

  it('propagates latency updates through subscriber snapshots', () => {
    const subscriber = vi.fn();
    service.connectTile('tile-latency', { sessionId: '', privateBeachId: null, managerUrl: '', authToken: '' }, subscriber);

    service.debugEmit('tile-latency', makeViewerState('connecting', { connecting: true }));
    service.debugEmit('tile-latency', makeViewerState('connected', { connecting: false, latencyMs: 128 }));
    service.debugEmit('tile-latency', makeViewerState('connected', { connecting: false, latencyMs: 256 }));

    const latencyValues = subscriber.mock.calls
      .map((call) => call[0]?.latencyMs)
      .filter((value) => value != null);
    expect(latencyValues).toEqual([128, 256]);

    const metricsSnapshot = service.getTileMetrics('tile-latency');
    expect(metricsSnapshot.completed).toBeGreaterThanOrEqual(1);
  });

  it('emits telemetry for connection lifecycle events', () => {
    service.connectTile(
      'tile-telemetry',
      { sessionId: 'tile-telemetry', privateBeachId: 'pb-test', managerUrl: 'https://manager.example', authToken: 'token' },
      vi.fn(),
    );

    service.debugEmit('tile-telemetry', makeViewerState('connecting', { connecting: true }));
    service.debugEmit('tile-telemetry', makeViewerState('connected', { connecting: false, latencyMs: 42 }));
    service.debugEmit('tile-telemetry', makeViewerState('error', { connecting: false, error: 'boom' }));

    const events = mockedTelemetry.mock.calls.map(([event, payload]) => ({ event, payload: payload as Record<string, unknown> }));
    expect(events.some((entry) => entry.event === 'canvas.tile.connect.start')).toBe(true);
    expect(events.some((entry) => entry.event === 'canvas.tile.connect.success')).toBe(true);
    expect(events.some((entry) => entry.event === 'canvas.tile.connect.failure')).toBe(true);
  });

  afterAll(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });
});

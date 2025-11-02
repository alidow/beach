import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from 'vitest';

vi.mock('../hooks/sessionTerminalManager', () => ({
  acquireTerminalConnection: vi.fn(() => () => {}),
  normalizeOverride: (override?: {
    passcode?: string | null;
    viewerToken?: string | null;
    authorizationToken?: string | null;
    skipCredentialFetch?: boolean;
  }) => ({
    passcode: override?.passcode ?? null,
    viewerToken: override?.viewerToken ?? null,
    authorizationToken: override?.authorizationToken ?? null,
    skipCredentialFetch: override?.skipCredentialFetch ?? false,
  }),
}));

const emitTelemetrySpy = vi.fn();

vi.mock('../../lib/telemetry', () => ({
  emitTelemetry: emitTelemetrySpy,
}));

import type { CanvasLayout } from '../../canvas';
import type { SessionSummary } from '../../lib/api';
import type { TerminalViewerState } from '../../hooks/terminalViewerTypes';
import type { TileMeasurementPayload } from '../sessionTileController';

let originalWindow: typeof window | undefined;

if (typeof window !== 'undefined') {
  originalWindow = window;
  Reflect.deleteProperty(globalThis, 'window');
}

vi.stubGlobal(
  'fetch',
  vi.fn(async (input: RequestInfo | URL) => {
    const url =
      typeof input === 'string'
        ? input
        : input instanceof URL
          ? input.href
          : typeof Request !== 'undefined' && input instanceof Request
            ? input.url
            : String(input);
    if (url.includes('/sessions/') && url.endsWith('/state')) {
      return new Response('{}', {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      });
    }
    if (url.includes('/viewer-credential')) {
      return new Response(
        JSON.stringify({
          token: 'stub-viewer-token',
          expiresAt: Date.now() + 60_000,
        }),
        {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        },
      );
    }
    return new Response(new Uint8Array(), { status: 200 });
  }),
);

const { sessionTileController } = await import('../sessionTileController');
const { viewerConnectionService } = await import('../viewerConnectionService');

const baseLayout: CanvasLayout = {
  version: 3,
  viewport: { zoom: 1, pan: { x: 0, y: 0 } },
  tiles: {
    'tile-1': {
      id: 'tile-1',
      kind: 'application',
      position: { x: 0, y: 0 },
      size: { width: 320, height: 240 },
      zIndex: 1,
      metadata: {},
    },
  },
  groups: {},
  agents: {},
  controlAssignments: {},
  metadata: { createdAt: Date.now(), updatedAt: Date.now() },
};

function makeSession(id: string): SessionSummary {
  return {
    session_id: id,
    private_beach_id: 'pb-1',
    harness_type: 'worker',
    capabilities: [],
    location_hint: null,
    metadata: {},
    version: '1',
    harness_id: `${id}-harness`,
    controller_token: null,
    controller_expires_at_ms: null,
    pending_actions: 0,
    pending_unacked: 0,
    last_health: null,
  };
}

function makeViewerState(
  status: TerminalViewerState['status'],
  overrides: Partial<TerminalViewerState & { transportVersion?: number }> = {},
): TerminalViewerState & { transportVersion?: number } {
  return {
    store: null,
    transport: null,
    connecting: status === 'connecting' || status === 'reconnecting',
    error: null,
    status,
    secureSummary: null,
    latencyMs: null,
    transportVersion: 0,
    ...overrides,
  };
}

function makeGridTileSnapshot({
  x,
  y,
  w,
  h,
  widthPx,
  heightPx,
  measurementVersion,
}: {
  x: number;
  y: number;
  w: number;
  h: number;
  widthPx: number;
  heightPx: number;
  measurementVersion: number;
}) {
  return {
    layout: { x, y, w, h },
    gridCols: 96,
    rowHeightPx: 12,
    layoutVersion: 2,
    widthPx,
    heightPx,
    zoom: 1,
    locked: false,
    toolbarPinned: false,
    manualLayout: true,
    hostCols: 80,
    hostRows: 24,
    measurementVersion,
    measurementSource: 'dom' as const,
    measurements: { width: widthPx, height: heightPx },
    viewportCols: 80,
    viewportRows: 24,
    layoutInitialized: true,
    layoutHostCols: 80,
    layoutHostRows: 24,
    hasHostDimensions: true,
    preview: null,
    previewStatus: 'ready' as const,
  };
}

function makeMeasurementPayload(overrides: Partial<TileMeasurementPayload> = {}): TileMeasurementPayload {
  return {
    scale: 1,
    targetWidth: 420,
    targetHeight: 300,
    rawWidth: 420,
    rawHeight: 300,
    hostRows: 24,
    hostCols: 80,
    measurementVersion: 1,
    ...overrides,
  };
}

function telemetryEvents(event: string) {
  return emitTelemetrySpy.mock.calls.filter(([name]) => name === event);
}

describe('SessionTileController lifecycle stress', () => {
  afterEach(() => {
    vi.useRealTimers();
    sessionTileController.resetViewerMetrics();
    sessionTileController.hydrate({
      layout: baseLayout,
      sessions: [],
      agents: [],
      privateBeachId: null,
      managerUrl: '',
      managerToken: null,
    });
    emitTelemetrySpy.mockClear();
  });

  afterAll(() => {
    vi.unstubAllGlobals();
    if (typeof originalWindow !== 'undefined') {
      (globalThis as any).window = originalWindow;
    } else {
      Reflect.deleteProperty(globalThis, 'window');
    }
  });

  it('debounces layout persistence under rapid updates', () => {
    vi.useFakeTimers();
    const onPersistLayout = vi.fn();
    const session = makeSession('tile-1');

    sessionTileController.hydrate({
      layout: baseLayout,
      sessions: [session],
      agents: [],
      privateBeachId: 'pb-1',
      managerUrl: 'http://localhost:8080',
      managerToken: null,
      onPersistLayout,
    });

    for (let i = 0; i < 5; i += 1) {
      sessionTileController.applyGridSnapshot('stress-update', {
        tiles: {
          'tile-1': {
            layout: { x: i, y: i % 2, w: 12, h: 8 },
            gridCols: 96,
            rowHeightPx: 12,
            layoutVersion: 2,
            widthPx: 400 + i,
            heightPx: 320,
            zoom: 1,
            locked: false,
            toolbarPinned: false,
            manualLayout: true,
            hostCols: 80,
            hostRows: 24,
            measurementVersion: i,
            measurementSource: 'dom',
            measurements: { width: 400 + i, height: 320 },
            viewportCols: 80,
            viewportRows: 24,
            layoutInitialized: true,
            layoutHostCols: 80,
            layoutHostRows: 24,
            hasHostDimensions: true,
            preview: null,
            previewStatus: 'ready',
          },
        },
        gridCols: 96,
        rowHeightPx: 12,
        layoutVersion: 2,
      });
    }

    expect(onPersistLayout).not.toHaveBeenCalled();

    vi.advanceTimersByTime(199);
    expect(onPersistLayout).not.toHaveBeenCalled();

    vi.advanceTimersByTime(2);
    expect(onPersistLayout).toHaveBeenCalledTimes(1);

    const persisted = onPersistLayout.mock.calls[0]?.[0];
    const layoutMetadata = persisted?.tiles?.['tile-1']?.metadata as any;
    expect(layoutMetadata?.dashboard?.layout?.x).toBe(4);
    expect(layoutMetadata?.dashboard?.widthPx).toBe(400 + 4);
  });

  it('applies the latest measurement when resize updates burst', () => {
    vi.useFakeTimers();
    const onPersistLayout = vi.fn();
    const session = makeSession('tile-1');

    sessionTileController.hydrate({
      layout: baseLayout,
      sessions: [session],
      agents: [],
      privateBeachId: 'pb-1',
      managerUrl: 'http://localhost:8080',
      managerToken: null,
      onPersistLayout,
    });

    for (let version = 1; version <= 5; version += 1) {
      sessionTileController.enqueueMeasurement(
        'tile-1',
        {
          scale: 0.8 + version * 0.02,
          targetWidth: 420 + version * 10,
          targetHeight: 300 + version * 8,
          rawWidth: 420 + version * 10,
          rawHeight: 300 + version * 8,
          hostRows: 24,
          hostCols: 80,
          measurementVersion: version,
        },
        'dom',
      );
    }

    vi.advanceTimersByTime(40);

    const snapshot = sessionTileController.getTileSnapshot('tile-1');
    expect(snapshot.layout?.metadata?.measurementVersion).toBe(5);
    expect(snapshot.layout?.metadata?.rawWidth).toBe(420 + 5 * 10);
    expect(snapshot.layout?.metadata?.rawHeight).toBe(300 + 5 * 8);
    vi.advanceTimersByTime(200);
    expect(onPersistLayout).not.toHaveBeenCalled();
  });

  it('prefers host measurements over DOM payloads when the version matches', () => {
    vi.useFakeTimers();
    const session = makeSession('tile-1');

    sessionTileController.hydrate({
      layout: baseLayout,
      sessions: [session],
      agents: [],
      privateBeachId: 'pb-1',
      managerUrl: 'http://localhost:8080',
      managerToken: null,
    });

    const domPayload = makeMeasurementPayload({
      measurementVersion: 12,
      rawWidth: 360,
      rawHeight: 260,
      targetWidth: 360,
      targetHeight: 260,
      scale: 0.87,
      hostRows: 28,
      hostCols: 70,
    });
    const hostPayload = makeMeasurementPayload({
      measurementVersion: 12,
      rawWidth: 408,
      rawHeight: 288,
      targetWidth: 408,
      targetHeight: 288,
      scale: 0.98,
      hostRows: 32,
      hostCols: 96,
    });

    sessionTileController.enqueueMeasurement('tile-1', domPayload, 'dom');
    sessionTileController.applyHostDimensions('tile-1', hostPayload);

    vi.advanceTimersByTime(40);

    const snapshot = sessionTileController.getTileSnapshot('tile-1');
    const metadata = snapshot.layout?.metadata as Record<string, unknown>;

    expect(metadata?.measurementVersion).toBe(hostPayload.measurementVersion);
    expect(metadata?.measurementSource).toBe('host');
    expect(metadata?.rawWidth).toBe(hostPayload.rawWidth);
    expect(metadata?.rawHeight).toBe(hostPayload.rawHeight);
    expect(metadata?.scale).toBe(hostPayload.scale);
    expect(metadata?.hostRows).toBe(hostPayload.hostRows);
    expect(metadata?.hostCols).toBe(hostPayload.hostCols);
    expect(Math.round(snapshot.layout?.size?.width ?? 0)).toBe(Math.round(hostPayload.rawWidth));
    expect(Math.round(snapshot.layout?.size?.height ?? 0)).toBe(Math.round(hostPayload.rawHeight));

    expect(snapshot.grid.measurementVersion).toBe(hostPayload.measurementVersion);
    expect(snapshot.grid.measurementSource).toBe('host');
    expect(snapshot.grid.widthPx).toBe(Math.round(hostPayload.rawWidth));
    expect(snapshot.grid.heightPx).toBe(Math.round(hostPayload.rawHeight));
    expect(snapshot.grid.hostRows).toBe(hostPayload.hostRows);
    expect(snapshot.grid.hostCols).toBe(hostPayload.hostCols);

    // Subsequent DOM payloads with the same version should be dropped in favour of cached host data.
    sessionTileController.enqueueMeasurement(
      'tile-1',
      makeMeasurementPayload({
        measurementVersion: hostPayload.measurementVersion,
        rawWidth: 300,
        rawHeight: 200,
        targetWidth: 300,
        targetHeight: 200,
        scale: 0.7,
        hostRows: 20,
        hostCols: 60,
      }),
      'dom',
    );

    vi.advanceTimersByTime(40);

    const persistedSnapshot = sessionTileController.getTileSnapshot('tile-1');
    const persistedMetadata = persistedSnapshot.layout?.metadata as Record<string, unknown>;

    expect(persistedMetadata?.measurementSource).toBe('host');
    expect(persistedMetadata?.rawWidth).toBe(hostPayload.rawWidth);
    expect(persistedSnapshot.grid.measurementSource).toBe('host');
    expect(persistedSnapshot.grid.widthPx).toBe(Math.round(hostPayload.rawWidth));
  });

  it('throttles persistence while tracking viewer metrics during connection churn', () => {
    vi.useFakeTimers();
    const onPersistLayout = vi.fn();
    const sessions = [makeSession('tile-1'), makeSession('tile-2')];
    const dualTileLayout: CanvasLayout = {
      ...baseLayout,
      tiles: {
        ...baseLayout.tiles,
        'tile-2': {
          id: 'tile-2',
          kind: 'application',
          position: { x: 8, y: 4 },
          size: { width: 300, height: 240 },
          zIndex: 2,
          metadata: {},
        },
      },
    };

    sessionTileController.resetViewerMetrics();
    sessionTileController.hydrate({
      layout: dualTileLayout,
      sessions,
      agents: [],
      privateBeachId: 'pb-1',
      managerUrl: 'http://localhost:8080',
      managerToken: 'token',
      viewerToken: 'viewer-token',
      onPersistLayout,
    });

    viewerConnectionService.debugEmit('tile-1', makeViewerState('connecting', { connecting: true }));
    viewerConnectionService.debugEmit('tile-1', makeViewerState('connected', { connecting: false }));
    viewerConnectionService.debugEmit('tile-2', makeViewerState('connecting', { connecting: true }));
    viewerConnectionService.debugEmit(
      'tile-2',
      makeViewerState('error', { connecting: false, error: 'keepalive timed out' }),
    );
    viewerConnectionService.debugEmit('tile-2', makeViewerState('reconnecting', { connecting: true }));
    viewerConnectionService.debugEmit('tile-2', makeViewerState('connected', { connecting: false, latencyMs: 96 }));

    for (let i = 0; i < 4; i += 1) {
      sessionTileController.applyGridSnapshot('grid-storm', {
        tiles: {
          'tile-1': makeGridTileSnapshot({
            x: i,
            y: i % 2,
            w: 12,
            h: 8,
            widthPx: 400 + i,
            heightPx: 320,
            measurementVersion: i,
          }),
          'tile-2': makeGridTileSnapshot({
            x: i + 1,
            y: (i + 1) % 2,
            w: 10,
            h: 6,
            widthPx: 360 + i,
            heightPx: 300,
            measurementVersion: i,
          }),
        },
        gridCols: 96,
        rowHeightPx: 12,
        layoutVersion: 2,
      });
    }

    expect(onPersistLayout).not.toHaveBeenCalled();
    vi.advanceTimersByTime(199);
    expect(onPersistLayout).not.toHaveBeenCalled();

    vi.advanceTimersByTime(2);
    expect(onPersistLayout).toHaveBeenCalledTimes(1);

    const persisted = onPersistLayout.mock.calls[0]?.[0];
    const tile1Layout = persisted?.tiles?.['tile-1']?.metadata as any;
    const tile2Layout = persisted?.tiles?.['tile-2']?.metadata as any;
    expect(tile1Layout?.dashboard?.layout?.x).toBe(3);
    expect(tile2Layout?.dashboard?.layout?.x).toBe(4);
    expect(tile1Layout?.dashboard?.widthPx).toBe(403);

    sessionTileController.applyGridSnapshot('grid-storm', {
      tiles: {
        'tile-1': makeGridTileSnapshot({
          x: 6,
          y: 0,
          w: 12,
          h: 8,
          widthPx: 412,
          heightPx: 320,
          measurementVersion: 9,
        }),
        'tile-2': makeGridTileSnapshot({
          x: 7,
          y: 1,
          w: 10,
          h: 6,
          widthPx: 372,
          heightPx: 300,
          measurementVersion: 9,
        }),
      },
      gridCols: 96,
      rowHeightPx: 12,
      layoutVersion: 2,
    });

    vi.advanceTimersByTime(199);
    expect(onPersistLayout).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(2);
    expect(onPersistLayout).toHaveBeenCalledTimes(2);

    const tile1Metrics = sessionTileController.getTileMetrics('tile-1');
    const tile2Metrics = sessionTileController.getTileMetrics('tile-2');

    expect(tile1Metrics.started).toBeGreaterThan(0);
    expect(tile1Metrics.completed).toBeGreaterThan(0);
    expect(tile1Metrics.retries).toBe(0);

    expect(tile2Metrics.started).toBeGreaterThan(0);
    expect(tile2Metrics.retries).toBeGreaterThan(0);
    expect(tile2Metrics.failures).toBeGreaterThan(0);
  });
});

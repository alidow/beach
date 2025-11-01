import { afterEach, describe, expect, it, vi } from 'vitest';

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

import type { CanvasLayout } from '../../canvas';
import type { SessionSummary } from '../../lib/api';

const { sessionTileController } = await import('../sessionTileController');

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

describe('SessionTileController lifecycle stress', () => {
  afterEach(() => {
    vi.useRealTimers();
    sessionTileController.hydrate({
      layout: baseLayout,
      sessions: [],
      agents: [],
      privateBeachId: null,
      managerUrl: '',
      managerToken: null,
    });
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
    expect(persisted?.tiles?.['tile-1']?.size?.width).toBe(400 + 4);
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
    expect(onPersistLayout).toHaveBeenCalledTimes(1);
  });
});

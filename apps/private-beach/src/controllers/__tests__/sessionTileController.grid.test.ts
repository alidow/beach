import { afterAll, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('fs/promises', () => ({
  readFile: async () => new Uint8Array([0]),
}));

vi.mock('argon2-browser/dist/argon2-bundled.min.js', () => ({
  default: {
    hash: async () => ({ hash: new Uint8Array(32) }),
    ArgonType: { Argon2id: 0 },
  },
}));

vi.stubGlobal(
  'fetch',
  vi.fn(async (input: RequestInfo | URL) => {
    const response: Response = {
      ok: true,
      status: 200,
      statusText: 'OK',
      headers: new Headers(),
      url: typeof input === 'string' ? input : input instanceof URL ? input.toString() : String(input),
      redirected: false,
      type: 'basic',
      clone() {
        return this;
      },
      arrayBuffer: async () => new ArrayBuffer(0),
      blob: async () => new Blob(),
      formData: async () => new FormData(),
      json: async () => ({}),
      text: async () => '',
      body: null,
      bodyUsed: false,
    };
    return response;
  }),
);

const { sessionTileController } = await import('../sessionTileController');
import type { GridLayoutSnapshot } from '../gridLayout';
import { applyGridDragCommand } from '../gridLayoutCommands';
import type { CanvasLayout } from '../../canvas';

function createBaseLayout(): CanvasLayout {
  const timestamp = Date.now();
  return {
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
    metadata: { createdAt: timestamp, updatedAt: timestamp },
  };
}

describe('SessionTileController grid helpers', () => {
  it('applies grid snapshot metadata to tiles', () => {
    sessionTileController.hydrate({
      layout: createBaseLayout(),
      sessions: [],
      agents: [],
      privateBeachId: null,
      managerUrl: '',
      managerToken: null,
    });

    const snapshot: GridLayoutSnapshot = {
      tiles: {
        'tile-1': {
          layout: { x: 8, y: 4, w: 16, h: 12 },
          gridCols: 128,
          rowHeightPx: 12,
          layoutVersion: 2,
          widthPx: 448,
          heightPx: 336,
          zoom: 0.9,
          locked: false,
          toolbarPinned: true,
          manualLayout: true,
          hostCols: 80,
          hostRows: 24,
          measurementVersion: 5,
          measurementSource: 'dom',
          measurements: { width: 448, height: 336 },
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
      gridCols: 128,
      rowHeightPx: 12,
      layoutVersion: 2,
    };

    sessionTileController.applyGridSnapshot('test-grid-update', snapshot, { suppressPersist: true });

    const tileSnapshot = sessionTileController.getTileSnapshot('tile-1');
    expect(tileSnapshot.grid.layout).toEqual({ x: 8, y: 4, w: 16, h: 12 });
    expect(tileSnapshot.grid.toolbarPinned).toBe(true);
    expect(tileSnapshot.grid.measurements).toEqual({ width: 448, height: 336 });
    expect(tileSnapshot.grid.previewStatus).toBe('ready');
    expect(tileSnapshot.grid.gridCols).toBe(128);
  });

  it('exports controller layout as BeachLayoutItems', () => {
    sessionTileController.hydrate({
      layout: createBaseLayout(),
      sessions: [],
      agents: [],
      privateBeachId: null,
      managerUrl: '',
      managerToken: null,
    });

    const snapshot: GridLayoutSnapshot = {
      tiles: {
        'tile-1': {
          layout: { x: 2, y: 3, w: 10, h: 6 },
          gridCols: 96,
          rowHeightPx: 16,
          layoutVersion: 2,
          widthPx: 400,
          heightPx: 300,
          zoom: 1,
          locked: false,
          toolbarPinned: false,
          manualLayout: true,
          hostCols: 72,
          hostRows: 20,
          measurementVersion: 1,
          measurementSource: 'dom',
          measurements: { width: 400, height: 300 },
          viewportCols: 72,
          viewportRows: 20,
          layoutInitialized: true,
          layoutHostCols: 72,
          layoutHostRows: 20,
          hasHostDimensions: true,
          preview: null,
          previewStatus: 'ready',
        },
      },
      gridCols: 96,
      rowHeightPx: 16,
      layoutVersion: 2,
    };

    sessionTileController.applyGridSnapshot('test-export', snapshot, { suppressPersist: true });

    const exported = sessionTileController.exportGridLayoutAsBeachItems();
    expect(exported).toEqual([
      expect.objectContaining({
        id: 'tile-1',
        x: 2,
        y: 3,
        w: 10,
        h: 6,
        widthPx: 400,
        heightPx: 300,
        gridCols: 96,
        rowHeightPx: 16,
        layoutVersion: 2,
        locked: false,
        toolbarPinned: false,
        zoom: 1,
      }),
    ]);
  });

  it('applies grid commands and schedules persistence', () => {
    vi.useFakeTimers();
    try {
      const onPersistLayout = vi.fn();
      sessionTileController.hydrate({
        layout: createBaseLayout(),
        sessions: [],
        agents: [],
        privateBeachId: null,
        managerUrl: '',
        managerToken: null,
        onPersistLayout,
      });

      sessionTileController.applyGridCommand(
        'test-command',
        (layout) =>
          applyGridDragCommand(
            layout,
            [{ i: 'tile-1', x: 12, y: 8, w: 20, h: 14 }],
            { cols: 128, rowHeightPx: 12 },
          ),
      );

      const snapshot = sessionTileController.getTileSnapshot('tile-1');
      expect(snapshot.grid.layout).toEqual({ x: 12, y: 8, w: 20, h: 14 });

      vi.advanceTimersByTime(250);
      expect(onPersistLayout).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it('suppresses persistence when grid command is applied with suppressPersist', () => {
    vi.useFakeTimers();
    try {
      const onPersistLayout = vi.fn();
      sessionTileController.hydrate({
        layout: createBaseLayout(),
        sessions: [],
        agents: [],
        privateBeachId: null,
        managerUrl: '',
        managerToken: null,
        onPersistLayout,
      });

      sessionTileController.applyGridCommand(
        'test-command-suppress',
        (layout) =>
          applyGridDragCommand(
            layout,
            [{ i: 'tile-1', x: 4, y: 2, w: 18, h: 12 }],
            { cols: 128, rowHeightPx: 12 },
          ),
        { suppressPersist: true },
      );

      const snapshot = sessionTileController.getTileSnapshot('tile-1');
      expect(snapshot.grid.layout).toEqual({ x: 4, y: 2, w: 18, h: 12 });

      vi.advanceTimersByTime(300);
      expect(onPersistLayout).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });
});

afterAll(() => {
  vi.unstubAllGlobals();
});

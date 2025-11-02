const measurementCallbacks = new Map<
  string,
  (sessionId: string, measurement: any) => void
>();

import React from 'react';
import { render, waitFor, act } from '@testing-library/react';
import { describe, expect, it, beforeEach, afterEach, vi } from 'vitest';

vi.mock('../../controllers/viewerConnectionService', () => {
  return {
    viewerConnectionService: {
      connectTile: vi.fn((_tileId: string, _input: unknown, subscriber: (snapshot: any) => void) => {
        subscriber({
          store: null,
          transport: null,
          connecting: false,
          error: null,
          status: 'idle',
          secureSummary: null,
          latencyMs: null,
        });
        return () => {};
      }),
      disconnectTile: vi.fn(),
      getTileMetrics: vi.fn(() => ({
        started: 0,
        completed: 0,
        retries: 0,
        failures: 0,
        disposed: 0,
      })),
      resetMetrics: vi.fn(),
    },
  };
});

vi.mock('next/dynamic', () => {
  const React = require('react') as typeof import('react');
  return {
    __esModule: true,
    default:
      (loader: () => Promise<any>) =>
      (props: Record<string, unknown>) => {
        const [Component, setComponent] = React.useState<React.ComponentType<any> | null>(null);
        React.useEffect(() => {
          let mounted = true;
          Promise.resolve(loader()).then((mod) => {
            if (!mounted) {
              return;
            }
            const resolved = mod.SessionTerminalPreview ?? mod.default ?? mod;
            setComponent(() => resolved);
          });
          return () => {
            mounted = false;
          };
        }, []);
        if (!Component) {
          return null;
        }
        const Resolved = Component;
        return <Resolved {...props} />;
      },
  };
});

vi.mock('../SessionTerminalPreview', () => {
  const React = require('react') as typeof import('react');
  return {
    __esModule: true,
    SessionTerminalPreview: ({
      sessionId,
      onPreviewMeasurementsChange,
    }: {
      sessionId: string;
      onPreviewMeasurementsChange?: (sessionId: string, measurement: any) => void;
    }) => {
      React.useEffect(() => {
        if (onPreviewMeasurementsChange) {
          measurementCallbacks.set(sessionId, onPreviewMeasurementsChange);
          return () => {
            measurementCallbacks.delete(sessionId);
          };
        }
        return;
      }, [sessionId, onPreviewMeasurementsChange]);
      return <div data-testid={`preview-${sessionId}`} />;
    },
  };
});

vi.mock('reactflow', () => {
  const React = require('react') as typeof import('react');
  const ReactFlowComponent = ({
    nodes,
    nodeTypes,
    children,
  }: {
    nodes: any[];
    nodeTypes: Record<string, React.ComponentType<any>>;
    children?: React.ReactNode;
  }) => (
    <div data-testid="reactflow">
      {nodes.map((node) => {
        const NodeComponent = nodeTypes[node.type];
        if (!NodeComponent) {
          return null;
        }
        return (
          <div key={node.id} data-testid={`node-${node.id}`}>
            <NodeComponent data={node.data} selected={Boolean(node.selected)} />
          </div>
        );
      })}
      {children}
    </div>
  );
  return {
    __esModule: true,
    default: ReactFlowComponent,
    ReactFlowProvider: ({ children }: { children: React.ReactNode }) => <>{children}</>,
    ReactFlow: ReactFlowComponent,
    Background: () => null,
    Controls: () => null,
    MiniMap: () => null,
    useReactFlow: () => ({
      project: ({ x, y }: { x: number; y: number }) => ({ x, y }),
      getViewport: () => ({ x: 0, y: 0, zoom: 1 }),
      fitView: () => {},
      setViewport: () => {},
    }),
    useOnViewportChange: () => {},
    applyNodeChanges: (_changes: unknown, nodes: unknown) => nodes,
  };
});

import CanvasSurface from '../CanvasSurface';
import { sessionTileController, type TileMeasurementPayload } from '../../controllers/sessionTileController';
import type { SessionSummary } from '../../lib/api';

function makeSession(overrides: Partial<SessionSummary> = {}): SessionSummary {
  return {
    session_id: 'tile-canvas-surface',
    private_beach_id: 'pb-1',
    harness_type: 'terminal',
    capabilities: [],
    location_hint: null,
    metadata: {},
    version: '1',
    harness_id: 'harness-0',
    controller_token: null,
    controller_expires_at_ms: null,
    pending_actions: 0,
    pending_unacked: 0,
    last_health: null,
    ...overrides,
  };
}

function makeLayout(tileId: string) {
  const timestamp = Date.now();
  return {
    version: 3,
    viewport: { zoom: 1, pan: { x: 0, y: 0 } },
    tiles: {
      [tileId]: {
        id: tileId,
        kind: 'application',
        position: { x: 0, y: 0 },
        size: { width: 420, height: 320 },
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

describe('CanvasSurface measurement parity', () => {
  const session = makeSession();
  const tileId = session.session_id;
  let originalFetch: typeof fetch | undefined;

  beforeEach(() => {
    measurementCallbacks.clear();
    originalFetch = globalThis.fetch;
    globalThis.fetch = vi.fn(async (input: RequestInfo | URL) => {
        const url =
          typeof input === 'string'
            ? input
            : input instanceof URL
              ? input.toString()
              : (input as Request).url;
        if (url.includes('/wasm/argon2.wasm')) {
          return {
            ok: true,
            arrayBuffer: async () => new ArrayBuffer(0),
          } as unknown as Response;
        }
        if (typeof originalFetch === 'function') {
          return originalFetch(input);
        }
        return {
          ok: false,
          status: 404,
          arrayBuffer: async () => new ArrayBuffer(0),
        } as unknown as Response;
      }) as typeof fetch;
    sessionTileController.hydrate({
      layout: makeLayout(tileId),
      sessions: [session],
      agents: [],
      privateBeachId: 'pb-1',
      managerUrl: 'http://localhost',
      managerToken: null,
      viewerToken: null,
    });
  });

  afterEach(() => {
    measurementCallbacks.clear();
    if (originalFetch) {
      globalThis.fetch = originalFetch;
    } else {
      delete (globalThis as any).fetch;
    }
    vi.restoreAllMocks();
  });

  it('uses host measurements for equal versions and ignores stale dom payloads', async () => {
    const layout = makeLayout(tileId);
    render(
      <CanvasSurface
        tiles={[session]}
        agents={[]}
        layout={layout as any}
        onLayoutChange={() => {}}
        onPersistLayout={() => {}}
        onRemove={() => {}}
        onSelect={() => {}}
        privateBeachId="pb-1"
        managerToken={null}
        managerUrl="http://localhost"
        viewerToken={null}
      />,
    );

    await waitFor(() => {
      expect(measurementCallbacks.has(tileId)).toBe(true);
    });
    const emitMeasurement = measurementCallbacks.get(tileId)!;

    const hostPayload: TileMeasurementPayload = {
      scale: 1,
      targetWidth: 500,
      targetHeight: 300,
      rawWidth: 500,
      rawHeight: 300,
      hostRows: 24,
      hostCols: 80,
      measurementVersion: 7,
    };

    act(() => {
      emitMeasurement(tileId, hostPayload);
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 60));
    });

    await waitFor(() => {
      const snapshot = sessionTileController.getTileSnapshot(tileId);
      expect(snapshot.layout?.metadata?.measurementSource).toBe('host');
      expect(snapshot.layout?.metadata?.rawWidth).toBe(500);
    });

    const domPayload: TileMeasurementPayload = {
      scale: 1,
      targetWidth: 480,
      targetHeight: 280,
      rawWidth: 480,
      rawHeight: 280,
      hostRows: null,
      hostCols: null,
      measurementVersion: 7,
    };

    act(() => {
      emitMeasurement(tileId, domPayload);
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 60));
    });

    const afterSnapshot = sessionTileController.getTileSnapshot(tileId);
    expect(afterSnapshot.layout?.metadata?.measurementSource).toBe('host');
    expect(afterSnapshot.layout?.metadata?.rawWidth).toBe(500);
    expect(afterSnapshot.layout?.size?.width).toBe(500);
    expect(afterSnapshot.layout?.size?.height).toBe(300);
  });
});

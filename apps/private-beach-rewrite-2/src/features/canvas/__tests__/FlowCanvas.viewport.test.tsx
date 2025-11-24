import { act, render } from '@testing-library/react';
import type { ReactNode } from 'react';
import { describe, expect, it, beforeEach, vi } from 'vitest';
import { CanvasEventsProvider } from '../CanvasEventsContext';
import { TileStoreProvider, useTileActions } from '@/features/tiles/store';
import type { TileState, TileViewportSnapshot } from '@/features/tiles/types';

vi.mock('@/hooks/useManagerToken', () => ({
  useManagerToken: () => ({
    token: 'manager-token',
    loading: false,
    error: null,
    isLoaded: true,
    isSignedIn: true,
    refresh: vi.fn(),
  }),
  buildManagerUrl: () => 'http://manager.test',
  buildRoadUrl: () => 'http://road.test',
}));

let lastNodes: unknown[] | null = null;

vi.mock('reactflow', () => {
  const React = require('react');
  const ReactFlow = ({ nodes, children }: { nodes: unknown[]; children?: ReactNode }) => {
    lastNodes = nodes;
    return <div data-testid="reactflow-mock">{children}</div>;
  };
  const Background = () => null;
  const ReactFlowProvider = ({ children }: { children: ReactNode }) => <>{children}</>;
  const Handle = ({ children }: { children?: ReactNode }) => <div data-testid="handle-mock">{children}</div>;
  const useReactFlow = () => ({
    screenToFlowPosition: (point: { x: number; y: number }) => point,
    zoomIn: () => {},
    zoomOut: () => {},
    fitView: () => {},
    setViewport: () => {},
  });
  const useStore = () => 1;
  return {
    __esModule: true,
    default: ReactFlow,
    ReactFlow,
    Background,
    ReactFlowProvider,
    Handle,
    useReactFlow,
    useStore,
    Position: { Left: 'left', Right: 'right', Top: 'top', Bottom: 'bottom' },
    MarkerType: { ArrowClosed: 'arrow-closed' },
    ConnectionMode: { Loose: 'loose', Strict: 'strict' },
    addEdge: (_edge: unknown, edges: unknown[]) => edges,
    applyEdgeChanges: (_changes: unknown, edges: unknown[]) => edges,
    useStoreApi: () => ({ getState: () => ({}) }),
  };
});

describe('FlowCanvas viewport updates', () => {
  let FlowCanvas: typeof import('../FlowCanvas')['FlowCanvas'];

  beforeAll(async () => {
    ({ FlowCanvas } = await import('../FlowCanvas'));
  });

  beforeEach(() => {
    lastNodes = null;
  });

  it('keeps node references stable when only viewport snapshots change', () => {
    let actions: ReturnType<typeof useTileActions> | null = null;

    function Harness({ children }: { children?: ReactNode }) {
      actions = useTileActions();
      return children ?? null;
    }

    const initialState: TileState = {
      tiles: {
        'tile-1': {
          id: 'tile-1',
          nodeType: 'application',
          position: { x: 0, y: 0 },
          size: { width: 320, height: 240 },
          sessionMeta: null,
          agentMeta: null,
          createdAt: 1,
          updatedAt: 1,
        },
      },
      order: ['tile-1'],
      relationships: {},
      relationshipOrder: [],
      activeId: null,
      resizing: {},
      interactiveId: null,
      viewport: {},
      canvasViewport: { zoom: 1, pan: { x: 0, y: 0 } },
    };

    const noop = () => {};

    render(
      <TileStoreProvider initialState={initialState}>
        <CanvasEventsProvider value={{ reportTileMove: noop }}>
          <Harness>
            <FlowCanvas
              onNodePlacement={noop}
              privateBeachId="beach-test"
              rewriteEnabled
            />
          </Harness>
        </CanvasEventsProvider>
      </TileStoreProvider>,
    );

    expect(lastNodes).toBeTruthy();
    const initialNodesRef = lastNodes;
    expect(initialNodesRef).toBeDefined();
    const snapshot: TileViewportSnapshot = {
      tileId: 'tile-1',
      hostRows: 40,
      hostCols: 120,
      viewportRows: 30,
      viewportCols: 100,
      pixelsPerRow: 12,
      pixelsPerCol: 6,
      hostWidthPx: null,
      hostHeightPx: null,
      cellWidthPx: 6,
      cellHeightPx: 12,
    };

    act(() => {
      actions?.updateTileViewport('tile-1', snapshot);
    });

    const nextNodesRef = lastNodes;
    expect(nextNodesRef).toBe(initialNodesRef);
  });
});

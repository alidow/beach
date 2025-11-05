"use client";

import { render, waitFor, cleanup } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useEffect } from 'react';
import { FlowCanvas } from '../FlowCanvas';
import { CanvasEventsProvider } from '../CanvasEventsContext';
import { TileStoreProvider, useTileActions } from '@/features/tiles';

const reactFlowTracker: { latestNodes: any[] } = { latestNodes: [] };

vi.mock('reactflow', () => {
  const React = require('react') as typeof import('react');
  const ReactFlowComponent = ({
    nodes,
    children,
    ...rest
  }: {
    nodes: any[];
    children?: React.ReactNode;
  }) => {
    reactFlowTracker.latestNodes = nodes;
    return (
      <div data-testid="mock-reactflow" data-props={JSON.stringify({ nodes, rest })}>
        {children}
      </div>
    );
  };
  return {
    __esModule: true,
    default: ReactFlowComponent,
    ReactFlow: ReactFlowComponent,
    ReactFlowProvider: ({ children }: { children: React.ReactNode }) => <>{children}</>,
    Background: () => null,
    Controls: () => null,
    useReactFlow: () => ({
      screenToFlowPosition: ({ x, y }: { x: number; y: number }) => ({ x, y }),
    }),
  };
});

vi.mock('@/features/tiles/components/TileFlowNode', () => {
  const React = require('react') as typeof import('react');
  return {
    __esModule: true,
    TileFlowNode: ({ data }: { data: any }) => (
      <div data-testid={`tile-node-${data.tile.id}`} data-node={JSON.stringify(data)} />
    ),
  };
});

function TileBootstrapper() {
  const { createTile } = useTileActions();
  useEffect(() => {
    createTile({
      id: 'tile-1',
      nodeType: 'application',
      position: { x: 128, y: 64 },
      size: { width: 400, height: 320 },
      focus: true,
    });
  }, [createTile]);
  return null;
}

function renderWithProviders(ui: React.ReactNode) {
  return render(
    <CanvasEventsProvider value={{ reportTileMove: () => {} }}>
      <TileStoreProvider>{ui}</TileStoreProvider>
    </CanvasEventsProvider>,
  );
}

function mockBoundingClientRect({
  width = 1024,
  height = 768,
  left = 32,
  top = 48,
}: {
  width?: number;
  height?: number;
  left?: number;
  top?: number;
} = {}) {
  return {
    width,
    height,
    top,
    left,
    right: left + width,
    bottom: top + height,
    x: left,
    y: top,
    toJSON() {
      return {};
    },
  } as DOMRect;
}

describe('FlowCanvas', () => {
  let rectSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    reactFlowTracker.latestNodes = [];
    rectSpy = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue(
      mockBoundingClientRect(),
    );
  });

  afterEach(() => {
    rectSpy.mockRestore();
    cleanup();
  });

  it('maps tiles from the store into React Flow nodes once the container is measured', async () => {
    renderWithProviders(
      <>
        <TileBootstrapper />
        <FlowCanvas
          onNodePlacement={vi.fn()}
          privateBeachId="pb-test"
          rewriteEnabled
          managerUrl="http://manager"
        />
      </>,
    );

    await waitFor(() => expect(reactFlowTracker.latestNodes.length).toBeGreaterThan(0));

    expect(reactFlowTracker.latestNodes).toHaveLength(1);
    expect(reactFlowTracker.latestNodes[0]).toMatchObject({
      id: 'tile-1',
      type: 'tile',
      position: { x: 128, y: 64 },
      style: expect.objectContaining({ width: 400, height: 320 }),
      data: expect.objectContaining({
        privateBeachId: 'pb-test',
        tile: expect.objectContaining({ id: 'tile-1' }),
      }),
    });
  });

  it('projects catalog drops into snapped canvas coordinates', async () => {
    const onPlacement = vi.fn();
    const { getByTestId } = renderWithProviders(
      <FlowCanvas
        onNodePlacement={onPlacement}
        privateBeachId="pb-drop"
        rewriteEnabled
        gridSize={8}
      />,
    );

    await waitFor(() => expect(reactFlowTracker.latestNodes).toEqual([]));

    const canvas = getByTestId('flow-canvas');

    const store: Record<string, string> = {};
    const dataTransfer = {
      dropEffect: 'none',
      effectAllowed: 'copy',
      files: [] as FileList,
      items: [] as DataTransferItemList,
      types: [] as string[],
      setData(type: string, value: string) {
        store[type] = value;
        if (!this.types.includes(type)) {
          this.types.push(type);
        }
      },
      getData(type: string) {
        return store[type] ?? '';
      },
      clearData(type?: string) {
        if (type) {
          delete store[type];
          this.types = this.types.filter((entry) => entry !== type);
        } else {
          for (const key of Object.keys(store)) {
            delete store[key];
          }
          this.types = [];
        }
      },
      setDragImage: vi.fn(),
    } as unknown as DataTransfer;

    dataTransfer.setData(
      'application/reactflow',
      JSON.stringify({
        id: 'application',
        nodeType: 'application',
        defaultSize: { width: 400, height: 320 },
      }),
    );
    dataTransfer.setData(
      'application/reactflow-offset',
      JSON.stringify({ x: 16, y: 12 }),
    );

    const dragOverEvent = new Event('dragover', { bubbles: true, cancelable: true }) as DragEvent;
    Object.assign(dragOverEvent, { dataTransfer });
    canvas.dispatchEvent(dragOverEvent);

    const dropEvent = new Event('drop', { bubbles: true, cancelable: true }) as DragEvent;
    Object.assign(dropEvent, { dataTransfer, clientX: 410, clientY: 250 });
    canvas.dispatchEvent(dropEvent);

    await waitFor(() => expect(onPlacement).toHaveBeenCalledTimes(1));
    expect(onPlacement).toHaveBeenCalledWith(
      expect.objectContaining({
        catalogId: 'application',
        nodeType: 'application',
        rawPosition: { x: 394, y: 238 },
        snappedPosition: { x: 392, y: 240 },
        gridSize: 8,
        canvasBounds: { width: 1024, height: 768 },
        source: 'catalog',
      }),
    );
  });
});

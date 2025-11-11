import { render } from '@testing-library/react';
import type { ReactNode } from 'react';
import { describe, expect, it, vi, beforeEach } from 'vitest';
import { FlowCanvas } from '../FlowCanvas';
import { TileStoreProvider } from '@/features/tiles/store';
import type { TileState } from '@/features/tiles/types';

const propsCapture = vi.hoisted(() => ({ last: null as Record<string, unknown> | null }));

vi.mock('reactflow', () => {
  const React = require('react');
  const { last } = propsCapture;
  const ReactFlow = (props: Record<string, unknown> & { children?: ReactNode }) => {
    propsCapture.last = props;
    return <div data-testid="reactflow-mock">{props.children}</div>;
  };
  const Background = () => null;
  const ReactFlowProvider = ({ children }: { children: ReactNode }) => <>{children}</>;
  const useReactFlow = () => ({ screenToFlowPosition: (p: { x: number; y: number }) => p });
  const useStore = () => 1;
  return { ReactFlow, Background, ReactFlowProvider, useReactFlow, useStore };
});

describe('FlowCanvas ReactFlow props', () => {
  beforeEach(() => {
    propsCapture.last = null;
  });

  it('sets anti-flicker and drag-related props', () => {
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

    render(
      <TileStoreProvider initialState={initialState}>
        <FlowCanvas onNodePlacement={() => {}} privateBeachId="beach-test" rewriteEnabled />
      </TileStoreProvider>,
    );

    const props = propsCapture.last as Record<string, unknown>;
    expect(props).toBeTruthy();
    expect(props.nodeDragHandle).toBe('.rf-drag-handle');
    expect(props.onlyRenderVisibleElements).toBe(false);
    expect(props.selectNodesOnDrag).toBe(false);
    expect(props.elevateNodesOnSelect).toBe(false);
    expect(props.nodesDraggable).toBe(true);
  });
});

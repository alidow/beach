'use client';

import {
  DndContext,
  DragOverlay,
  PointerSensor,
  useSensor,
  useSensors,
  type DragEndEvent,
  type DragStartEvent,
} from '@dnd-kit/core';
import { useCallback, useMemo, useRef, useState, type ReactNode } from 'react';
import { CanvasUIProvider } from './CanvasContext';
import { CanvasEventsProvider, type TileMoveReport } from './CanvasEventsContext';
import { NodeCatalogPreview, NodeDrawer } from './NodeDrawer';
import { CanvasSurface, CANVAS_SURFACE_ID } from './CanvasSurface';
import type { CanvasNodeDefinition, NodePlacementPayload, TileMovePayload } from './types';

type CanvasWorkspaceProps = {
  nodes: CanvasNodeDefinition[];
  onNodePlacement: (payload: NodePlacementPayload) => void;
  onTileMove?: (payload: TileMovePayload) => void;
  children?: ReactNode;
  initialDrawerOpen?: boolean;
  gridSize?: number;
};

const DEFAULT_GRID_SIZE = 8;

function snapToGrid(value: number, grid: number) {
  if (grid <= 0) return value;
  return Math.round(value / grid) * grid;
}

function clamp(value: number, min: number, max: number) {
  return Math.min(Math.max(value, min), max);
}

export function CanvasWorkspace({
  nodes,
  onNodePlacement,
  onTileMove,
  children,
  initialDrawerOpen = true,
  gridSize = DEFAULT_GRID_SIZE,
}: CanvasWorkspaceProps) {
  const canvasRef = useRef<HTMLDivElement | null>(null);
  const [activeNode, setActiveNode] = useState<CanvasNodeDefinition | null>(null);

  const sensors = useSensors(
    useSensor(PointerSensor, {
      activationConstraint: {
        distance: 6,
      },
    }),
  );

  const handleDragStart = useCallback((event: DragStartEvent) => {
    const node = event.active.data.current?.catalogNode as CanvasNodeDefinition | undefined;
    if (node) {
      setActiveNode(node);
    }
  }, []);

  const handleDragEnd = useCallback(
    (event: DragEndEvent) => {
      const node = event.active.data.current?.catalogNode as CanvasNodeDefinition | undefined;
      setActiveNode(null);

      const canvasEl = canvasRef.current;
      if (!canvasEl) {
        return;
      }

      const canvasRect = canvasEl.getBoundingClientRect();
      const translatedRect = event.active.rect.current.translated ?? event.active.rect.current.initial;

      if (!translatedRect) {
        return;
      }

      if (node && event.over?.id === CANVAS_SURFACE_ID) {
        const nodeWidth = node.defaultSize.width;
        const nodeHeight = node.defaultSize.height;

        const rawX = translatedRect.left - canvasRect.left;
        const rawY = translatedRect.top - canvasRect.top;

        const snappedX = snapToGrid(rawX, gridSize);
        const snappedY = snapToGrid(rawY, gridSize);

        const maxX = Math.max(0, canvasRect.width - nodeWidth);
        const maxY = Math.max(0, canvasRect.height - nodeHeight);

        const clampedX = clamp(snappedX, 0, maxX);
        const clampedY = clamp(snappedY, 0, maxY);

        onNodePlacement({
          catalogId: node.id,
          nodeType: node.nodeType,
          size: { width: nodeWidth, height: nodeHeight },
          rawPosition: { x: rawX, y: rawY },
          snappedPosition: { x: clampedX, y: clampedY },
          canvasBounds: { width: canvasRect.width, height: canvasRect.height },
          gridSize,
          source: 'catalog',
        });
        return;
      }
    },
    [gridSize, onNodePlacement],
  );

  const handleDragCancel = useCallback(() => {
    setActiveNode(null);
  }, []);

  const activeNodeId = activeNode?.id ?? null;

  const activePreview = useMemo(() => {
    if (!activeNode) return null;
    return <NodeCatalogPreview node={activeNode} />;
  }, [activeNode]);

  const eventsValue = useMemo(
    () => ({
      reportTileMove: (event: TileMoveReport) => {
        if (!onTileMove) return;
        const canvasRect = canvasRef.current?.getBoundingClientRect();
        onTileMove({
          tileId: event.tileId,
          source: event.source,
          rawPosition: event.rawPosition,
          snappedPosition: event.snappedPosition,
          delta: {
            x: event.snappedPosition.x - event.originalPosition.x,
            y: event.snappedPosition.y - event.originalPosition.y,
          },
          canvasBounds: canvasRect
            ? { width: canvasRect.width, height: canvasRect.height }
            : { width: 0, height: 0 },
          gridSize,
          timestamp: Date.now(),
        });
      },
    }),
    [gridSize, onTileMove],
  );

  return (
    <CanvasEventsProvider value={eventsValue}>
      <CanvasUIProvider initialDrawerOpen={initialDrawerOpen}>
        <DndContext
          sensors={sensors}
          onDragStart={handleDragStart}
          onDragEnd={handleDragEnd}
          onDragCancel={handleDragCancel}
        >
          <div className="flex w-full flex-1 gap-4">
            <CanvasSurface ref={canvasRef}>{children}</CanvasSurface>
            <NodeDrawer nodes={nodes} activeNodeId={activeNodeId} />
          </div>
          <DragOverlay dropAnimation={null}>{activePreview}</DragOverlay>
        </DndContext>
      </CanvasUIProvider>
    </CanvasEventsProvider>
  );
}

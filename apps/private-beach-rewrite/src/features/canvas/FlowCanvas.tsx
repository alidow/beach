'use client';

import 'reactflow/dist/style.css';

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import ReactFlow, {
  Background,
  ReactFlowProvider,
  useReactFlow,
  Controls,
  type Node,
  type NodeChange,
  type NodeDragEventHandler,
  type NodeResizeEvent,
} from 'reactflow';
import { emitTelemetry } from '../../../../private-beach/src/lib/telemetry';
import { TileFlowNode } from '@/features/tiles/components/TileFlowNode';
import { TILE_GRID_SNAP_PX } from '@/features/tiles/constants';
import { useTileActions, useTileState } from '@/features/tiles/store';
import { snapSize } from '@/features/tiles/utils';
import { buildManagerUrl } from '@/hooks/useManagerToken';
import { useCanvasEvents } from './CanvasEventsContext';
import type { CanvasNodeDefinition, CanvasPoint, NodePlacementPayload, TileMovePayload } from './types';

const APPLICATION_MIME = 'application/reactflow';
const OFFSET_MIME = 'application/reactflow-offset';
const nodeTypes = { tile: TileFlowNode };

type FlowCanvasProps = {
  onNodePlacement: (payload: NodePlacementPayload) => void;
  onTileMove?: (payload: TileMovePayload) => void;
  privateBeachId: string;
  managerUrl?: string;
  rewriteEnabled: boolean;
  gridSize?: number;
};

type DragSnapshot = {
  tileId: string;
  originalPosition: CanvasPoint;
};

type CatalogDragPayload = Pick<CanvasNodeDefinition, 'id' | 'nodeType' | 'defaultSize'>;

function snapPoint(point: CanvasPoint, gridSize: number): CanvasPoint {
  if (gridSize <= 0) return point;
  const snap = (value: number) => Math.round(value / gridSize) * gridSize;
  return { x: snap(point.x), y: snap(point.y) };
}

function clampPosition(position: CanvasPoint, size: { width: number; height: number }, bounds: CanvasPoint) {
  return {
    x: Math.min(Math.max(position.x, 0), Math.max(0, bounds.x - size.width)),
    y: Math.min(Math.max(position.y, 0), Math.max(0, bounds.y - size.height)),
  };
}

function FlowCanvasInner({
  onNodePlacement,
  onTileMove,
  privateBeachId,
  managerUrl,
  rewriteEnabled,
  gridSize = TILE_GRID_SNAP_PX,
}: FlowCanvasProps) {
  const wrapperRef = useRef<HTMLDivElement | null>(null);
  const dragSnapshotRef = useRef<DragSnapshot | null>(null);
  const { screenToFlowPosition } = useReactFlow();
  const state = useTileState();
  const { setTilePosition, bringToFront, setActiveTile, resizeTile, beginResize, endResize } = useTileActions();
  const { reportTileMove } = useCanvasEvents();
  const [containerReady, setContainerReady] = useState(false);

  const resolvedManagerUrl = useMemo(() => buildManagerUrl(managerUrl), [managerUrl]);

  const nodes: Node[] = useMemo(() => {
    return state.order
      .map((tileId, index) => {
        const tile = state.tiles[tileId];
        if (!tile) return null;
        return {
          id: tile.id,
          type: 'tile',
          data: {
            tile,
            orderIndex: index,
            isActive: state.activeId === tile.id,
            isResizing: Boolean(state.resizing[tile.id]),
            privateBeachId,
            managerUrl: resolvedManagerUrl,
            rewriteEnabled,
          },
          position: tile.position,
          draggable: true,
          selectable: false,
          connectable: false,
          style: {
            width: tile.size.width,
            height: tile.size.height,
            zIndex: 10 + index,
          },
        } satisfies Node;
      })
      .filter((node): node is Node => Boolean(node));
  }, [privateBeachId, resolvedManagerUrl, rewriteEnabled, state]);

  const handleNodesChange = useCallback(
    (changes: NodeChange[]) => {
      changes.forEach((change) => {
        if (change.type !== 'position' || !change.position) {
          return;
        }
        const snapped = snapPoint(change.position, gridSize);
        const tile = state.tiles[change.id];
        if (!tile) return;
        if (snapped.x === tile.position.x && snapped.y === tile.position.y) {
          return;
        }
        setTilePosition(change.id, snapped);
      });
    },
    [gridSize, setTilePosition, state.tiles],
  );

  const handleNodeDragStart: NodeDragEventHandler = useCallback(
    (_event, node) => {
      const tile = state.tiles[node.id];
      if (!tile) return;
      bringToFront(node.id);
      setActiveTile(node.id);
      dragSnapshotRef.current = {
        tileId: node.id,
        originalPosition: { ...tile.position },
      };
      emitTelemetry('canvas.drag.start', {
        privateBeachId,
        tileId: node.id,
        nodeType: tile.nodeType,
        x: tile.position.x,
        y: tile.position.y,
        rewriteEnabled,
      });
    },
    [bringToFront, privateBeachId, rewriteEnabled, setActiveTile, state.tiles],
  );

  const handleNodeDragStop: NodeDragEventHandler = useCallback(
    (_event, node) => {
      const tile = state.tiles[node.id];
      const snapshot = dragSnapshotRef.current;
      dragSnapshotRef.current = null;
      if (!tile || !snapshot || snapshot.tileId !== node.id) {
        return;
      }
      const snappedPosition = snapPoint(node.position, gridSize);
      const delta = {
        x: snappedPosition.x - snapshot.originalPosition.x,
        y: snappedPosition.y - snapshot.originalPosition.y,
      };

      const wrapperBounds = wrapperRef.current?.getBoundingClientRect();
      const canvasBounds = wrapperBounds ? { width: wrapperBounds.width, height: wrapperBounds.height } : { width: 0, height: 0 };

      const payload: TileMovePayload = {
        tileId: node.id,
        source: 'pointer',
        rawPosition: node.position,
        snappedPosition,
        delta,
        canvasBounds,
        gridSize,
        timestamp: Date.now(),
      };

      onTileMove?.(payload);

      reportTileMove({
        tileId: node.id,
        size: { ...tile.size },
        originalPosition: snapshot.originalPosition,
        rawPosition: node.position,
        snappedPosition,
        source: 'pointer',
      });

      emitTelemetry('canvas.drag.stop', {
        privateBeachId,
        tileId: node.id,
        nodeType: tile.nodeType,
        x: snappedPosition.x,
        y: snappedPosition.y,
        rewriteEnabled,
      });
    },
    [gridSize, onTileMove, privateBeachId, reportTileMove, rewriteEnabled, state.tiles],
  );


  const handleNodeResizeStart = useCallback((event: NodeResizeEvent, node: Node) => {
    bringToFront(node.id);
    setActiveTile(node.id);
    beginResize(node.id);
  }, [beginResize, bringToFront, setActiveTile]);

  const handleNodeResize = useCallback((event: NodeResizeEvent, node: Node) => {
    const tile = state.tiles[node.id];
    if (!tile) return;
    const width = event.width ?? node.width ?? tile.size.width;
    const height = event.height ?? node.height ?? tile.size.height;
    const snapped = snapSize({ width, height });
    resizeTile(node.id, snapped);
  }, [resizeTile, state.tiles]);

  const handleNodeResizeStop = useCallback((event: NodeResizeEvent, node: Node) => {
    const tile = state.tiles[node.id];
    if (!tile) return;
    const width = event.width ?? node.width ?? tile.size.width;
    const height = event.height ?? node.height ?? tile.size.height;
    const snapped = snapSize({ width, height });
    resizeTile(node.id, snapped);
    endResize(node.id);
    emitTelemetry('canvas.resize.stop', {
      privateBeachId,
      tileId: node.id,
      width: snapped.width,
      height: snapped.height,
      rewriteEnabled,
    });
  }, [endResize, privateBeachId, resizeTile, rewriteEnabled, state.tiles]);

  const handleDrop = useCallback(
    (event: DragEvent) => {
      const descriptor = event.dataTransfer?.getData(APPLICATION_MIME);
      if (!descriptor) {
        return;
      }

      event.preventDefault();

      let payload: CatalogDragPayload | null = null;
      try {
        payload = JSON.parse(descriptor) as CatalogDragPayload;
      } catch {
        payload = null;
      }
      if (!payload) return;

      let pointerOffset = { x: 0, y: 0 };
      const offsetRaw = event.dataTransfer?.getData(OFFSET_MIME);
      if (offsetRaw) {
        try {
          const parsed = JSON.parse(offsetRaw) as { x: number; y: number };
          pointerOffset = {
            x: Number.isFinite(parsed.x) ? parsed.x : 0,
            y: Number.isFinite(parsed.y) ? parsed.y : 0,
          };
        } catch {
          pointerOffset = { x: 0, y: 0 };
        }
      }

      const screenPoint = { x: event.clientX - pointerOffset.x, y: event.clientY - pointerOffset.y };
      const flowPosition = screenToFlowPosition(screenPoint);
      const snapped = snapPoint(flowPosition, gridSize);

      const container = wrapperRef.current;
      const bounds = container?.getBoundingClientRect();
      const width = bounds?.width ?? 0;
      const height = bounds?.height ?? 0;

      let flowBounds: CanvasPoint = { x: width, y: height };
      if (bounds) {
        const topLeftFlow = screenToFlowPosition({ x: bounds.left, y: bounds.top });
        const bottomRightFlow = screenToFlowPosition({ x: bounds.left + bounds.width, y: bounds.top + bounds.height });
        flowBounds = {
          x: bottomRightFlow.x - topLeftFlow.x,
          y: bottomRightFlow.y - topLeftFlow.y,
        };
      }

      const clamped = clampPosition(snapped, payload.defaultSize, flowBounds);

      onNodePlacement({
        catalogId: payload.id,
        nodeType: payload.nodeType,
        size: { width: payload.defaultSize.width, height: payload.defaultSize.height },
        rawPosition: flowPosition,
        snappedPosition: clamped,
        canvasBounds: { width, height },
        gridSize,
        source: 'catalog',
      });
    },
    [gridSize, onNodePlacement, screenToFlowPosition],
  );

  const handleDragOver = useCallback((event: DragEvent) => {
    const types = event.dataTransfer ? Array.from(event.dataTransfer.types) : [];
    if (types.includes(APPLICATION_MIME)) {
      event.preventDefault();
      event.dataTransfer.dropEffect = 'copy';
    }
  }, []);

  useEffect(() => {
    const node = wrapperRef.current;
    if (!node) return;
    const onDragOver = (event: DragEvent) => handleDragOver(event);
    const onDrop = (event: DragEvent) => handleDrop(event);
    node.addEventListener('dragover', onDragOver);
    node.addEventListener('drop', onDrop);
    return () => {
      node.removeEventListener('dragover', onDragOver);
      node.removeEventListener('drop', onDrop);
    };
  }, [handleDragOver, handleDrop]);

  useEffect(() => {
    const node = wrapperRef.current;
    if (!node) {
      setContainerReady(false);
      return;
    }

    const MIN_READY_HEIGHT = 48;

    const applyMeasurement = (rect: DOMRectReadOnly | DOMRect) => {
      const ready = rect.width > 0 && rect.height > MIN_READY_HEIGHT;
      console.info('[ws-c][flow] container-measure', {
        width: rect.width,
        height: rect.height,
        ready,
      });
      setContainerReady(ready);
    };

    applyMeasurement(node.getBoundingClientRect());

    if (typeof ResizeObserver !== 'undefined') {
      const observer = new ResizeObserver((entries) => {
        const entry = entries[0];
        if (!entry) return;
        const rect = entry.contentRect ?? node.getBoundingClientRect();
        applyMeasurement(rect);
      });
      observer.observe(node);
      return () => {
        observer.disconnect();
      };
    }

    let rafId: number | null = null;
    const poll = () => {
      applyMeasurement(node.getBoundingClientRect());
      rafId = requestAnimationFrame(poll);
    };
    rafId = requestAnimationFrame(poll);
    return () => {
      if (rafId !== null) {
        cancelAnimationFrame(rafId);
      }
    };
  }, []);

  return (
    <div
      ref={wrapperRef}
      className="relative flex h-full flex-1 w-full overflow-hidden rounded-xl border border-border bg-card/60 shadow-inner"
      data-testid="flow-canvas"
      style={{ minHeight: '100%', height: '100%' }}
    >
      {containerReady ? (
        <ReactFlow
          nodes={nodes}
          edges={[]}
          nodeTypes={nodeTypes}
          onNodesChange={handleNodesChange}
          onNodeDragStart={handleNodeDragStart}
          onNodeDragStop={handleNodeDragStop}
          nodesDraggable
          nodesConnectable={false}
          elementsSelectable={false}
          panOnScroll
          panOnDrag
          zoomOnScroll
          zoomOnPinch
          zoomOnDoubleClick={false}
          minZoom={0.25}
          maxZoom={2}
          fitView={false}
          elevateNodesOnSelect
          proOptions={{ hideAttribution: true }}
          className="flex-1 h-full w-full"
          style={{ width: '100%', height: '100%' }}
          nodesResizable
          onNodeResize={handleNodeResize}
          onNodeResizeStart={handleNodeResizeStart}
          onNodeResizeStop={handleNodeResizeStop}
        >
          <Background gap={gridSize} color="rgba(148, 163, 184, 0.18)" />
          <Controls
            showInteractive={false}
            position="bottom-right"
            className="bg-card/80 text-foreground"
          />
        </ReactFlow>
      ) : (
        <div className="flex h-full w-full items-center justify-center text-xs text-muted-foreground">Preparing canvas…</div>
      )}
    </div>
  );
}

export function FlowCanvas(props: FlowCanvasProps) {
  return (
    <ReactFlowProvider>
      <FlowCanvasInner {...props} />
    </ReactFlowProvider>
  );
}

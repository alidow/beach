'use client';

import 'reactflow/dist/style.css';

import { useCallback, useEffect, useMemo, useRef } from 'react';
import ReactFlow, {
  Background,
  ReactFlowProvider,
  useReactFlow,
  useStore,
  type ReactFlowState,
  type Node,
  type NodeChange,
  type NodeDragEventHandler,
} from 'reactflow';
import { emitTelemetry } from '../../../../private-beach/src/lib/telemetry';
import { TileFlowNode } from '@/features/tiles/components/TileFlowNode';
import { TILE_GRID_SNAP_PX } from '@/features/tiles/constants';
import { useTileActions, useTileState } from '@/features/tiles/store';
import { buildManagerUrl } from '@/hooks/useManagerToken';
import { useCanvasEvents } from './CanvasEventsContext';
import type { CanvasNodeDefinition, CanvasPoint, NodePlacementPayload, TileMovePayload } from './types';

const APPLICATION_MIME = 'application/reactflow';
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

const zoomSelector = (state: ReactFlowState) => state.transform[2] ?? 1;

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
  const flow = useReactFlow();
  const { screenToFlowPosition } = flow;
  const state = useTileState();
  const { setTilePosition, bringToFront, setActiveTile } = useTileActions();
  const { reportTileMove } = useCanvasEvents();

  const resolvedManagerUrl = useMemo(() => buildManagerUrl(managerUrl), [managerUrl]);

  const nodes: Node[] = useMemo(() => {
    return state.order
      .map((tileId, index) => {
        const tile = state.tiles[tileId];
        if (!tile) return null;
        const isInteractive = state.interactiveId === tile.id;
        return {
          id: tile.id,
          type: 'tile',
          data: {
            tile,
            orderIndex: index,
            isActive: state.activeId === tile.id,
            isResizing: Boolean(state.resizing[tile.id]),
            isInteractive,
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
        if (change.type === 'position' && change.position) {
          const snapped = snapPoint(change.position, gridSize);
          const tile = state.tiles[change.id];
          if (!tile) return;
          if (snapped.x === tile.position.x && snapped.y === tile.position.y) {
            return;
          }
          setTilePosition(change.id, snapped);
        }
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

  const handleNodeDrag: NodeDragEventHandler = useCallback(
    (_event, node) => {
      const tile = state.tiles[node.id];
      if (!tile) return;
      const snapped = snapPoint(node.position, gridSize);
      if (snapped.x === tile.position.x && snapped.y === tile.position.y) {
        return;
      }
      setTilePosition(node.id, snapped);
    },
    [gridSize, setTilePosition, state.tiles],
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
      setTilePosition(node.id, snappedPosition);

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
    [gridSize, onTileMove, privateBeachId, reportTileMove, rewriteEnabled, setTilePosition, state.tiles],
  );

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

      const flowPosition = screenToFlowPosition({ x: event.clientX, y: event.clientY });
      const snapped = snapPoint(flowPosition, gridSize);

      const container = wrapperRef.current;
      const bounds = container?.getBoundingClientRect();
      const width = bounds?.width ?? 0;
      const height = bounds?.height ?? 0;
      const clamped = clampPosition(snapped, payload.defaultSize, { x: width, y: height });

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
    if (event.dataTransfer?.types.includes(APPLICATION_MIME)) {
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

  return (
    <div
      ref={wrapperRef}
      className="relative flex-1 h-full w-full overflow-hidden bg-slate-950/40 backdrop-blur-2xl"
      data-testid="flow-canvas"
    >
      <ReactFlow
        nodes={nodes}
        edges={[]}
        nodeTypes={nodeTypes}
        onNodesChange={handleNodesChange}
        onNodeDrag={handleNodeDrag}
        onNodeDragStart={handleNodeDragStart}
        onNodeDragStop={handleNodeDragStop}
        nodesDraggable
        nodesConnectable={false}
        elementsSelectable={false}
        panOnScroll={false}
        panOnDrag={false}
        zoomOnScroll={false}
        zoomOnPinch
        zoomOnDoubleClick={false}
        fitView={false}
        elevateNodesOnSelect
        proOptions={{ hideAttribution: true }}
        className="h-full w-full"
        minZoom={0.2}
        maxZoom={1.75}
        style={{ width: '100%', height: '100%' }}
      >
        <Background gap={gridSize} color="rgba(56, 189, 248, 0.12)" />
      </ReactFlow>
      <CanvasViewportControls />
    </div>
  );
}

function CanvasViewportControls() {
  const flow = useReactFlow();
  const zoom = useStore(zoomSelector);
  const zoomPercent = Math.round((zoom ?? 1) * 100);

  const handleZoomIn = useCallback(() => {
    flow.zoomIn({ duration: 160 });
  }, [flow]);

  const handleZoomOut = useCallback(() => {
    flow.zoomOut({ duration: 160 });
  }, [flow]);

  const handleFitView = useCallback(() => {
    flow.fitView({ padding: 0.16, duration: 200 });
  }, [flow]);

  return (
    <div className="pointer-events-auto absolute bottom-5 left-5 z-30 flex items-center gap-2 rounded-full border border-white/10 bg-slate-950/80 px-3 py-1.5 text-[11px] font-semibold uppercase tracking-[0.24em] text-slate-300 shadow-[0_12px_40px_rgba(2,6,23,0.55)] backdrop-blur">
      <button
        type="button"
        onClick={handleZoomOut}
        className="inline-flex h-7 w-7 items-center justify-center rounded-full border border-white/10 bg-white/5 text-sm text-slate-200 transition hover:border-white/25 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-400/60"
        aria-label="Zoom out"
      >
        −
      </button>
      <span className="min-w-[3ch] text-center text-[11px] text-white/80">{zoomPercent}%</span>
      <button
        type="button"
        onClick={handleZoomIn}
        className="inline-flex h-7 w-7 items-center justify-center rounded-full border border-white/10 bg-white/5 text-sm text-slate-200 transition hover:border-white/25 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-400/60"
        aria-label="Zoom in"
      >
        +
      </button>
      <button
        type="button"
        onClick={handleFitView}
        className="ml-1 inline-flex h-7 items-center justify-center rounded-full border border-white/10 bg-white/5 px-2 text-[10px] text-slate-300 transition hover:border-white/25 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-400/60"
      >
        Fit
      </button>
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

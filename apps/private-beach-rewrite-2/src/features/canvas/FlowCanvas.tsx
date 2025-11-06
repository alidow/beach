'use client';

import 'reactflow/dist/style.css';

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import ReactFlow, {
  Background,
  ReactFlowProvider,
  addEdge,
  applyEdgeChanges,
  useReactFlow,
  useStore,
  type Connection,
  type Edge,
  type EdgeChange,
  type Node,
  type NodeChange,
  type NodeDragEventHandler,
  type ReactFlowState,
} from 'reactflow';
import { emitTelemetry } from '../../../../private-beach/src/lib/telemetry';
import { TileFlowNode } from '@/features/tiles/components/TileFlowNode';
import { TILE_GRID_SNAP_PX } from '@/features/tiles/constants';
import { useTileActions, useTileState } from '@/features/tiles/store';
import { buildManagerUrl } from '@/hooks/useManagerToken';
import { useCanvasEvents } from './CanvasEventsContext';
import { clampPointToBounds, snapPointToGrid } from './positioning';
import { AssignmentEdge, type AssignmentEdgeData, type UpdateMode } from './AssignmentEdge';
import type {
  CanvasBounds,
  CanvasNodeDefinition,
  CanvasPoint,
  NodePlacementPayload,
  TileMovePayload,
} from './types';

const APPLICATION_MIME = 'application/reactflow';
const nodeTypes = { tile: TileFlowNode };
const edgeTypes = { assignment: AssignmentEdge };

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
  const canvasBoundsRef = useRef<CanvasBounds | null>(null);
  const [canvasBounds, setCanvasBounds] = useState<CanvasBounds | null>(null);
  const [edges, setEdges] = useState<Array<Edge<AssignmentEdgeData>>>([]);
  const flow = useReactFlow();
  const { screenToFlowPosition } = flow;
  const state = useTileState();
  const { setTilePosition, setTilePositionImmediate, bringToFront, setActiveTile } = useTileActions();
  const { reportTileMove } = useCanvasEvents();

  const applyCanvasBounds = useCallback((bounds: CanvasBounds | null) => {
    canvasBoundsRef.current = bounds;
    setCanvasBounds(bounds);
  }, []);

  const readCanvasBounds = useCallback((): CanvasBounds | null => {
    if (canvasBoundsRef.current) {
      return canvasBoundsRef.current;
    }
    const rect = wrapperRef.current?.getBoundingClientRect();
    if (!rect) {
      return null;
    }
    const bounds = { width: rect.width, height: rect.height };
    applyCanvasBounds(bounds);
    return bounds;
  }, [applyCanvasBounds]);

  const clampToCanvas = useCallback(
    (position: CanvasPoint, size: { width: number; height: number }): CanvasPoint => {
      const bounds = readCanvasBounds();
      if (!bounds) {
        return position;
      }
      return clampPointToBounds(position, size, bounds);
    },
    [readCanvasBounds],
  );

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
          connectable: true,
          style: {
            width: tile.size.width,
            height: tile.size.height,
            zIndex: 10 + index,
          },
        } satisfies Node;
      })
      .filter((node): node is Node => Boolean(node));
  }, [privateBeachId, resolvedManagerUrl, rewriteEnabled, state]);

  const handleEdgeSave = useCallback(
    ({ id, instructions, updateMode, pollFrequency }: { id: string; instructions: string; updateMode: UpdateMode; pollFrequency: number }) => {
      setEdges((current) =>
        current.map((edge) =>
          edge.id === id
            ? {
                ...edge,
                data: { ...edge.data, instructions, updateMode, pollFrequency, isEditing: false },
              }
            : edge,
        ),
      );
    },
    [],
  );

  const handleEdgeEdit = useCallback(({ id }: { id: string }) => {
    setEdges((current) =>
      current.map((edge) =>
        edge.id === id ? { ...edge, data: { ...edge.data, isEditing: true } } : edge,
      ),
    );
  }, []);

  const handleEdgeDelete = useCallback(({ id }: { id: string }) => {
    setEdges((current) => current.filter((edge) => edge.id !== id));
  }, []);

  const handleNodesChange = useCallback(
    (changes: NodeChange[]) => {
      changes.forEach((change) => {
        if (change.type === 'position' && change.position) {
          const tile = state.tiles[change.id];
          if (!tile) return;
          if (change.dragging) {
            const clamped = clampToCanvas(change.position, tile.size);
            setTilePositionImmediate(change.id, clamped);
            return;
          }
          const snapped = snapPointToGrid(change.position, gridSize);
          const clamped = clampToCanvas(snapped, tile.size);
          if (clamped.x === tile.position.x && clamped.y === tile.position.y) {
            return;
          }
          setTilePosition(change.id, clamped);
        }
      });
    },
    [clampToCanvas, gridSize, setTilePosition, setTilePositionImmediate, state.tiles],
  );

  const handleEdgesChange = useCallback(
    (changes: EdgeChange[]) => setEdges((eds) => applyEdgeChanges(changes, eds)),
    [],
  );

  const handleConnect = useCallback(
    (connection: Connection) => {
      if (!connection.source || !connection.target) {
        return;
      }
      const sourceTile = state.tiles[connection.source];
      const targetTile = state.tiles[connection.target];
      if (!sourceTile || !targetTile) {
        return;
      }
      if (sourceTile.nodeType !== 'agent') {
        return;
      }
      const edgeId = `assignment-${Date.now()}-${Math.round(Math.random() * 1000)}`;
      const edge: Edge<AssignmentEdgeData> = {
        id: edgeId,
        type: 'assignment',
        source: connection.source,
        target: connection.target,
        data: {
          instructions: '',
          updateMode: 'idle-summary',
          pollFrequency: 60,
          isEditing: true,
          onSave: handleEdgeSave,
          onEdit: handleEdgeEdit,
          onDelete: handleEdgeDelete,
        },
      };
      setEdges((current) => addEdge(edge, current));
    },
    [handleEdgeDelete, handleEdgeEdit, handleEdgeSave, state.tiles],
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
      const clamped = clampToCanvas(node.position, tile.size);
      setTilePositionImmediate(node.id, clamped);
    },
    [clampToCanvas, setTilePositionImmediate, state.tiles],
  );

  const handleNodeDragStop: NodeDragEventHandler = useCallback(
    (_event, node) => {
      const tile = state.tiles[node.id];
      const snapshot = dragSnapshotRef.current;
      dragSnapshotRef.current = null;
      if (!tile || !snapshot || snapshot.tileId !== node.id) {
        return;
      }
      const snappedPosition = snapPointToGrid(node.position, gridSize);
      const clampedPosition = clampToCanvas(snappedPosition, tile.size);
      setTilePosition(node.id, clampedPosition);

      const delta = {
        x: clampedPosition.x - snapshot.originalPosition.x,
        y: clampedPosition.y - snapshot.originalPosition.y,
      };

      const bounds = readCanvasBounds();
      const canvasBounds = bounds ?? { width: 0, height: 0 };

      const payload: TileMovePayload = {
        tileId: node.id,
        source: 'pointer',
        rawPosition: node.position,
        snappedPosition: clampedPosition,
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
        snappedPosition: clampedPosition,
        source: 'pointer',
      });

      emitTelemetry('canvas.drag.stop', {
        privateBeachId,
        tileId: node.id,
        nodeType: tile.nodeType,
        x: clampedPosition.x,
        y: clampedPosition.y,
        rewriteEnabled,
      });
    },
    [clampToCanvas, gridSize, onTileMove, privateBeachId, readCanvasBounds, reportTileMove, rewriteEnabled, setTilePosition, state.tiles],
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
      const snapped = snapPointToGrid(flowPosition, gridSize);

      const bounds = readCanvasBounds();
      const width = bounds?.width ?? 0;
      const height = bounds?.height ?? 0;
      const clamped = bounds ? clampPointToBounds(snapped, payload.defaultSize, bounds) : snapped;

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
    [gridSize, onNodePlacement, readCanvasBounds, screenToFlowPosition],
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

  useEffect(() => {
    const node = wrapperRef.current;
    if (!node) {
      return undefined;
    }
    const rect = node.getBoundingClientRect();
    applyCanvasBounds({ width: rect.width, height: rect.height });
    const observer = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (!entry) {
        return;
      }
      applyCanvasBounds({
        width: entry.contentRect.width,
        height: entry.contentRect.height,
      });
    });
    observer.observe(node);
    return () => {
      observer.disconnect();
    };
  }, [applyCanvasBounds]);

  useEffect(() => {
    setEdges((current) => current.filter((edge) => state.tiles[edge.source] && state.tiles[edge.target]));
  }, [state.tiles]);

  const nodeExtent = useMemo(() => {
    if (!canvasBounds) {
      return undefined;
    }
    return [
      [0, 0],
      [canvasBounds.width, canvasBounds.height],
    ] as [[number, number], [number, number]];
  }, [canvasBounds]);

  return (
    <div
      ref={wrapperRef}
      className="relative flex-1 h-full w-full overflow-hidden bg-slate-950/40 backdrop-blur-2xl"
      data-testid="flow-canvas"
    >
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        edgeTypes={edgeTypes}
        onNodesChange={handleNodesChange}
        onEdgesChange={handleEdgesChange}
        onConnect={handleConnect}
        onNodeDrag={handleNodeDrag}
        onNodeDragStart={handleNodeDragStart}
        onNodeDragStop={handleNodeDragStop}
        nodesDraggable
        nodesConnectable
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
        nodeExtent={nodeExtent}
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

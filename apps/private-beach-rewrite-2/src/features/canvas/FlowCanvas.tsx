'use client';

import 'reactflow/dist/style.css';
/**
 * FlowCanvas
 *
 * Dragging stability notes for future maintainers/agents:
 *
 * 1) Use header-only drag via `nodeDragHandle=".rf-drag-handle"` and make sure
 *    the node header element has the `rf-drag-handle` class. Do NOT disable
 *    `nodesDraggable` conditionally; gate the start area instead.
 *
 * 2) Keep external state in sync during drag: in onNodesChange, when
 *    `change.dragging` is true, call `setTilePositionImmediate(id, position)`.
 *    Commit snapped position only on onNodeDragStop. Avoid writing positions in
 *    onNodeDrag to prevent extra renders.
 *
 * 3) Anti-flicker React Flow props: keep
 *    - onlyRenderVisibleElements={false}
 *    - elevateNodesOnSelect={false}
 *    - selectNodesOnDrag={false}
 *    - panOnDrag (pane-only; nodes use header handle)
 *    - ReactFlow style includes translateZ(0) + willChange
 *
 * 4) Avoid main-thread work during drag: do not auto-persist layout mid-drag.
 *    See useTileLayoutPersistence({ auto: false }) and persist only on drop.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import ReactFlow, {
  Background,
  ReactFlowProvider,
  MarkerType,
  useReactFlow,
  useStore,
  type Connection,
  type Edge,
  type Node,
  type NodeChange,
  type NodeDragEventHandler,
  type ReactFlowState,
  type Viewport,
} from 'reactflow';
import { emitTelemetry } from '../../../../private-beach/src/lib/telemetry';
import { TileFlowNode } from '@/features/tiles/components/TileFlowNode';
import { TILE_GRID_SNAP_PX } from '@/features/tiles/constants';
import { useTileActions, useTileState } from '@/features/tiles/store';
import type { CanvasViewportState, RelationshipDescriptor } from '@/features/tiles/types';
import {
  acquireController,
  batchControllerAssignments,
  deleteControllerPairing,
  type ControllerUpdateCadence,
} from '@/lib/api';
import { recordTraceLog, useTraceLogs, clearTraceLogs } from '@/features/trace/traceLogStore';
import { buildManagerUrl, useManagerToken } from '@/hooks/useManagerToken';
import { useCanvasEvents } from './CanvasEventsContext';
import { snapPointToGrid } from './positioning';
import { AssignmentEdge, type AssignmentEdgeData, type UpdateMode } from './AssignmentEdge';
import { TraceMonitorPanel } from './TraceMonitorPanel';
import { CanvasDragStateProvider } from './CanvasDragStateContext';
import { CANVAS_CENTER_TILE_EVENT, type CanvasCenterTileEventDetail } from './events';
import type {
  CanvasBounds,
  CanvasNodeDefinition,
  CanvasPoint,
  NodePlacementPayload,
  TileMovePayload,
} from './types';

const APPLICATION_MIME = 'application/reactflow';
// Stable type maps: never recreate between renders (prevents RF #002).
const NODE_TYPES = Object.freeze({ tile: TileFlowNode });
const EDGE_TYPES = Object.freeze({ assignment: AssignmentEdge });
const defaultEdgeOptions = {
  type: 'smoothstep' as const,
  markerEnd: {
    type: MarkerType.ArrowClosed,
    color: '#94a3b8',
    width: 18,
    height: 18,
  },
};

const CONTROLLER_LEASE_TTL_MS = 120_000;
const CONTROLLER_LEASE_REFRESH_BUFFER_MS = 5_000;
const VIEWPORT_PAN_EPSILON = 0.5;
const VIEWPORT_ZOOM_EPSILON = 0.0005;

type FlowCanvasProps = {
  onNodePlacement: (payload: NodePlacementPayload) => void;
  onTileMove?: (payload: TileMovePayload) => void;
  onViewportChange?: (viewport: CanvasViewportState) => void;
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

type PairingHistoryEntry = {
  id: string;
  status: 'ok' | 'error';
  message: string | null;
  timestamp: number;
};

function FlowCanvasInner({
  onNodePlacement,
  onTileMove,
  onViewportChange,
  privateBeachId,
  managerUrl,
  rewriteEnabled,
  gridSize = TILE_GRID_SNAP_PX,
}: FlowCanvasProps) {
  const wrapperRef = useRef<HTMLDivElement | null>(null);
  const dragSnapshotRef = useRef<DragSnapshot | null>(null);
  const canvasBoundsRef = useRef<CanvasBounds | null>(null);
  const relationshipSyncKeysRef = useRef<Record<string, string>>({});
  const previousRelationshipsRef = useRef<Record<string, RelationshipDescriptor>>({});
  const [relationshipErrors, setRelationshipErrors] = useState<Record<string, string>>({});
  const [relationshipSyncHistory, setRelationshipSyncHistory] = useState<Record<string, PairingHistoryEntry[]>>({});
  const [syncNonce, setSyncNonce] = useState(0);
  const [traceOverlay, setTraceOverlay] = useState<{
    relationshipId: string;
    traceId: string;
    instruction: string;
    cadence: ControllerUpdateCadence | null;
    pollFrequency: number | null;
  } | null>(null);
  const traceLogs = useTraceLogs(traceOverlay?.traceId ?? null);
  const [editingEdgeId, setEditingEdgeId] = useState<string | null>(null);
  const [dragCount, setDragCount] = useState(0);
  const { token: managerToken, refresh: refreshManagerToken } = useManagerToken();
  const controllerLeaseExpiryRef = useRef<Record<string, number>>({});
  const state = useTileState();
  const { order, tiles, activeId, resizing, interactiveId, canvasViewport } = state;
  const {
    setTilePosition,
    setTilePositionImmediate,
    bringToFront,
    setActiveTile,
    addRelationship,
    updateRelationship,
    removeRelationship,
    setCanvasViewport,
  } = useTileActions();
  const flow = useReactFlow();
  const { screenToFlowPosition } = flow;
  const lastViewportPublishRef = useRef<CanvasViewportState>(canvasViewport);
  const appliedViewportRef = useRef<CanvasViewportState | null>(null);
  const tilesRef = useRef(state.tiles);
  useEffect(() => {
    tilesRef.current = state.tiles;
  }, [state.tiles]);
  useEffect(() => {
    lastViewportPublishRef.current = canvasViewport;
  }, [canvasViewport]);
  useEffect(() => {
    const desired = canvasViewport;
    if (!desired) {
      return;
    }
    const applied = appliedViewportRef.current;
    if (
      applied &&
      Math.abs(applied.zoom - desired.zoom) < VIEWPORT_ZOOM_EPSILON &&
      Math.abs(applied.pan.x - desired.pan.x) < VIEWPORT_PAN_EPSILON &&
      Math.abs(applied.pan.y - desired.pan.y) < VIEWPORT_PAN_EPSILON
    ) {
      return;
    }
    flow.setViewport({ x: desired.pan.x, y: desired.pan.y, zoom: desired.zoom }, { duration: applied ? 160 : 0 });
    appliedViewportRef.current = desired;
  }, [canvasViewport, flow]);
  const memoizedNodeTypes = useMemo(() => NODE_TYPES, []);
  const memoizedEdgeTypes = useMemo(() => EDGE_TYPES, []);
  // Runtime guard to surface accidental remount causes during dev
  if (process.env.NODE_ENV !== 'production') {
    try {
      if (!memoizedNodeTypes || !memoizedEdgeTypes) {
        // noop, keeps bundlers from tree-shaking the guard
      }
    } catch {}
  }
  const { reportTileMove } = useCanvasEvents();

  useEffect(() => {
    if (typeof window === 'undefined') {
      return undefined;
    }
    const handleCenterTile = (event: Event) => {
      const detail = (event as CustomEvent<CanvasCenterTileEventDetail>).detail;
      const tileId = detail?.tileId;
      if (!tileId) {
        return;
      }
      const tile = tilesRef.current[tileId];
      if (!tile) {
        return;
      }
      const centerX = tile.position.x + tile.size.width / 2;
      const centerY = tile.position.y + tile.size.height / 2;
      const zoom = flow.getZoom();
      flow.setCenter(centerX, centerY, { zoom, duration: 220 });
    };
    window.addEventListener(CANVAS_CENTER_TILE_EVENT, handleCenterTile as EventListener);
    return () => {
      window.removeEventListener(CANVAS_CENTER_TILE_EVENT, handleCenterTile as EventListener);
    };
  }, [flow]);

  const applyCanvasBounds = useCallback((bounds: CanvasBounds | null) => {
    canvasBoundsRef.current = bounds;
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

  const resolvedManagerUrl = useMemo(() => buildManagerUrl(managerUrl), [managerUrl]);

  const ensureControllerLease = useCallback(
    async (controllerSessionId: string, authToken: string) => {
      if (!controllerSessionId) {
        throw new Error('controller session id missing for lease acquisition');
      }
      const now = Date.now();
      const expiry = controllerLeaseExpiryRef.current[controllerSessionId];
      if (expiry && expiry - now > CONTROLLER_LEASE_REFRESH_BUFFER_MS) {
        return;
      }
      try {
        const lease = await acquireController(
          controllerSessionId,
          CONTROLLER_LEASE_TTL_MS,
          authToken,
          resolvedManagerUrl,
        );
        const nextExpiry = lease.expires_at_ms ?? now + CONTROLLER_LEASE_TTL_MS;
        controllerLeaseExpiryRef.current[controllerSessionId] = nextExpiry;
      } catch (error) {
        // In dev, some manager builds return HTTP 409 (Conflict) when the DB
        // migrations for controller leases are out of date. Since pairing does
        // not strictly require a lease on the controller session (the batch
        // assignment API performs its own auth), treat 409 as non-fatal so the
        // canvas can proceed without surfacing a blocking error.
        const message = error instanceof Error ? error.message : String(error);
        if (message.includes('409')) {
          const fallbackExpiry = now + CONTROLLER_LEASE_TTL_MS;
          controllerLeaseExpiryRef.current[controllerSessionId] = fallbackExpiry;
          console.info('[rewrite-2] proceeding without controller lease due to 409');
          return;
        }
        console.warn('[rewrite-2] failed to acquire controller lease', {
          controllerSessionId,
          error,
        });
        throw error;
      }
    },
    [resolvedManagerUrl],
  );

  // Cache node objects: unchanged tiles keep stable references across drag frames.
  const nodeCacheRef = useRef<Map<string, { node: Node; sig: string }>>(new Map());
  const nodes: Node[] = useMemo(() => {
    const next: Node[] = [];
    const cache = nodeCacheRef.current;
    const seen = new Set<string>();
    for (let index = 0; index < order.length; index += 1) {
      const tileId = order[index];
      const tile = tiles[tileId];
      if (!tile) continue;
      const isInteractive = interactiveId === tile.id;
      const sessionMetaSig = tile.sessionMeta
        ? [
            tile.sessionMeta.sessionId ?? '',
            tile.sessionMeta.title ?? '',
            tile.sessionMeta.status ?? '',
            tile.sessionMeta.harnessType ?? '',
            tile.sessionMeta.pendingActions ?? '',
          ].join('~')
        : 'session:none';
      const agentMetaSig = tile.agentMeta
        ? [
            tile.agentMeta.role ?? '',
            tile.agentMeta.responsibility ?? '',
            tile.agentMeta.isEditing ? 'editing' : 'saved',
            tile.agentMeta.trace?.enabled ? 'trace:on' : 'trace:off',
            tile.agentMeta.trace?.trace_id ?? '',
          ].join('~')
        : 'agent:none';
      const sig = [
        tile.id,
        tile.position.x,
        tile.position.y,
        tile.size.width,
        tile.size.height,
        index,
        activeId === tile.id ? 1 : 0,
        resizing[tile.id] ? 1 : 0,
        isInteractive ? 1 : 0,
        privateBeachId,
        resolvedManagerUrl,
        rewriteEnabled ? 1 : 0,
        sessionMetaSig,
        agentMetaSig,
      ].join('|');
      const cached = cache.get(tile.id);
      if (cached && cached.sig === sig) {
        next.push(cached.node);
        seen.add(tile.id);
        continue;
      }
      const node: Node = {
        id: tile.id,
        type: 'tile',
        data: {
          tile,
          orderIndex: index,
          isActive: activeId === tile.id,
          isResizing: Boolean(resizing[tile.id]),
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
      };
      cache.set(tile.id, { node, sig });
      next.push(node);
      seen.add(tile.id);
    }
    // Prune removed tiles
    for (const key of Array.from(cache.keys())) {
      if (!seen.has(key)) {
        cache.delete(key);
      }
    }
    return next;
  }, [activeId, interactiveId, order, privateBeachId, resolvedManagerUrl, resizing, rewriteEnabled, tiles]);

  const handleEdgeSave = useCallback(
    ({ id, instructions, updateMode, pollFrequency }: { id: string; instructions: string; updateMode: UpdateMode; pollFrequency: number }) => {
      const relationship = state.relationships[id];
      const sourceTile = relationship ? state.tiles[relationship.sourceId] : undefined;
      const targetTile = relationship ? state.tiles[relationship.targetId] : undefined;
      delete relationshipSyncKeysRef.current[id];
      setRelationshipErrors((prev) => {
        if (!(id in prev)) {
          return prev;
        }
        const next = { ...prev };
        delete next[id];
        return next;
      });
      updateRelationship(id, { instructions, updateMode, pollFrequency });
      setEditingEdgeId(null);
      if (!relationship || !sourceTile || !targetTile) {
        console.warn('[rewrite-2] missing relationship context for edge save', { relationshipId: id });
        return;
      }
      const controllerSessionId = sourceTile.sessionMeta?.sessionId;
      const childSessionId = targetTile.sessionMeta?.sessionId;
      if (!controllerSessionId || !childSessionId) {
        console.warn('[rewrite-2] edge save missing session ids', {
          relationshipId: id,
          controllerSessionId,
          childSessionId,
        });
        return;
      }
      const role = sourceTile.agentMeta?.role?.trim() ?? '';
      const responsibility = sourceTile.agentMeta?.responsibility?.trim() ?? '';
      const trimmedInstructions = instructions.trim();
      if (!role || !responsibility || !trimmedInstructions) {
        console.warn('[rewrite-2] edge save missing prompt context', {
          relationshipId: id,
          role,
          responsibility,
          hasInstructions: Boolean(trimmedInstructions),
        });
        return;
      }
      if (!privateBeachId) {
        console.warn('[rewrite-2] private beach id missing for edge save');
        return;
      }
      const promptTemplate = buildPromptTemplate(role, responsibility, trimmedInstructions);
      const updateCadence = mapUpdateModeToCadence(updateMode);
      const traceId =
        sourceTile.agentMeta?.trace?.enabled && sourceTile.agentMeta.trace.trace_id
          ? sourceTile.agentMeta.trace.trace_id
          : null;
      void (async () => {
        try {
          const authToken = managerToken ?? (await refreshManagerToken());
          if (!authToken) {
            console.warn('[rewrite-2] missing manager token for batch assignment', { relationshipId: id });
            setRelationshipErrors((prev) => ({
              ...prev,
              [id]: 'Manager token unavailable. Please sign in again.',
            }));
            return;
          }
          await ensureControllerLease(controllerSessionId, authToken);
          await batchControllerAssignments(
            privateBeachId,
            [
              {
                controller_session_id: controllerSessionId,
                child_session_id: childSessionId,
                prompt_template: promptTemplate,
                update_cadence: updateCadence,
              },
            ],
            authToken,
            resolvedManagerUrl,
            traceId ?? undefined,
          );
          console.info('[rewrite-2] controller assignment saved', {
            relationshipId: id,
            controller_session_id: controllerSessionId,
            child_session_id: childSessionId,
          });
          setRelationshipErrors((prev) => {
            if (!(id in prev)) {
              return prev;
            }
            const next = { ...prev };
            delete next[id];
            return next;
          });
        } catch (error) {
          const message = error instanceof Error ? error.message : 'Failed to sync assignment';
          console.warn('[rewrite-2] failed to save controller assignment', {
            relationshipId: id,
            error,
          });
          setRelationshipErrors((prev) => ({ ...prev, [id]: message }));
        }
      })();
    },
    [
      ensureControllerLease,
      managerToken,
      privateBeachId,
      refreshManagerToken,
      resolvedManagerUrl,
      setRelationshipErrors,
      state.relationships,
      state.tiles,
      updateRelationship,
    ],
  );

  const handleViewportMoveEnd = useCallback(
    (_event: MouseEvent | TouchEvent | null, viewport: Viewport) => {
      if (!viewport) {
        return;
      }
      const normalizeNumber = (value: number | undefined, fallback: number) =>
        typeof value === 'number' && Number.isFinite(value) ? value : fallback;
      const nextViewport: CanvasViewportState = {
        zoom: Math.min(1.75, Math.max(0.2, normalizeNumber(viewport.zoom, canvasViewport.zoom))),
        pan: {
          x: normalizeNumber(viewport.x, canvasViewport.pan.x),
          y: normalizeNumber(viewport.y, canvasViewport.pan.y),
        },
      };
      const prev = lastViewportPublishRef.current;
      const hasMeaningfulDelta =
        Math.abs(prev.zoom - nextViewport.zoom) > VIEWPORT_ZOOM_EPSILON ||
        Math.abs(prev.pan.x - nextViewport.pan.x) > VIEWPORT_PAN_EPSILON ||
        Math.abs(prev.pan.y - nextViewport.pan.y) > VIEWPORT_PAN_EPSILON;
      if (!hasMeaningfulDelta) {
        return;
      }
      lastViewportPublishRef.current = nextViewport;
      setCanvasViewport(nextViewport);
      onViewportChange?.(nextViewport);
    },
    [canvasViewport.pan.x, canvasViewport.pan.y, canvasViewport.zoom, onViewportChange, setCanvasViewport],
  );

  const handleEdgeEdit = useCallback(({ id }: { id: string }) => {
    setEditingEdgeId(id);
  }, []);

  const handleShowTraceOverlay = useCallback(
    ({ id }: { id: string }) => {
      const relationship = state.relationships[id];
      if (!relationship) {
        return;
      }
      const sourceTile = state.tiles[relationship.sourceId];
      const traceId =
        sourceTile && sourceTile.agentMeta?.trace?.enabled ? sourceTile.agentMeta.trace.trace_id ?? null : null;
      if (!traceId) {
        return;
      }
      setTraceOverlay({
        relationshipId: id,
        traceId,
        instruction: relationship.instructions,
        cadence: mapUpdateModeToCadence(relationship.updateMode as UpdateMode),
        pollFrequency: relationship.pollFrequency ?? null,
      });
    },
    [state.relationships, state.tiles],
  );

  const teardownRelationshipByData = useCallback(
    async (relationship?: RelationshipDescriptor, traceId?: string | null) => {
      if (!relationship) {
        return;
      }
      if (relationship.id) {
        delete relationshipSyncKeysRef.current[relationship.id];
      }
      if (!relationship.sourceSessionId || !relationship.targetSessionId) {
        return;
      }
      const authToken = managerToken ?? (await refreshManagerToken());
      if (!authToken) {
        return;
      }
      if (traceId) {
        recordTraceLog(traceId, {
          source: 'assignments',
          level: 'info',
          message: 'Deleting controller pairing',
          detail: {
            controller_session_id: relationship.sourceSessionId ?? null,
            child_session_id: relationship.targetSessionId ?? null,
            relationship_id: relationship.id,
          },
        });
      }
      try {
        await deleteControllerPairing(
          relationship.sourceSessionId,
          relationship.targetSessionId,
          authToken,
          resolvedManagerUrl,
          traceId ?? undefined,
        );
        if (traceId) {
          recordTraceLog(traceId, {
            source: 'assignments',
            level: 'info',
            message: 'Controller pairing deleted',
            detail: {
              controller_session_id: relationship.sourceSessionId ?? null,
              child_session_id: relationship.targetSessionId ?? null,
              relationship_id: relationship.id,
            },
          });
        }
      } catch (error) {
        if (traceId) {
          recordTraceLog(traceId, {
            source: 'assignments',
            level: 'error',
            message: 'Failed to delete controller pairing',
            detail: {
              controller_session_id: relationship.sourceSessionId ?? null,
              child_session_id: relationship.targetSessionId ?? null,
              relationship_id: relationship.id,
              error: error instanceof Error ? error.message : String(error),
            },
          });
        }
        console.warn('[rewrite-2] failed to delete controller pairing', error);
      }
    },
    [managerToken, refreshManagerToken, resolvedManagerUrl],
  );

  const handleEdgeDelete = useCallback(
    ({ id }: { id: string }) => {
      const relationship = state.relationships[id];
      const traceId =
        relationship && state.tiles[relationship.sourceId]?.agentMeta?.trace?.enabled
          ? state.tiles[relationship.sourceId]?.agentMeta?.trace?.trace_id ?? null
          : null;
      void teardownRelationshipByData(relationship, traceId);
      removeRelationship(id);
      delete relationshipSyncKeysRef.current[id];
      previousRelationshipsRef.current = { ...previousRelationshipsRef.current };
      delete previousRelationshipsRef.current[id];
      setRelationshipErrors((prev) => {
        if (!(id in prev)) {
          return prev;
        }
        const next = { ...prev };
        delete next[id];
        return next;
      });
      setEditingEdgeId((current) => (current === id ? null : current));
    },
    [removeRelationship, state.relationships, state.tiles, teardownRelationshipByData],
  );

  const handleNodesChange = useCallback(
    (changes: NodeChange[]) => {
      changes.forEach((change) => {
        if (change.type === 'position' && change.position) {
          const tile = state.tiles[change.id];
          if (!tile) return;
          // IMPORTANT: Keep controlled node positions in sync while dragging.
          // RF moves nodes via CSS transforms. If our props lag behind,
          // RF may reconcile to the old position and flicker.
          if (change.dragging) {
            const px = Math.round(change.position.x);
            const py = Math.round(change.position.y);
            setTilePositionImmediate(change.id, { x: px, y: py });
            if ((window as any).__PB_DEBUG_DRAG) {
              try {
                console.info('[rf][drag] frame', {
                  id: change.id,
                  x: px,
                  y: py,
                });
              } catch {}
            }
            return;
          }
          const snapped = snapPointToGrid(change.position, gridSize);
          if (snapped.x === tile.position.x && snapped.y === tile.position.y) {
            return;
          }
          setTilePosition(change.id, snapped);
          if ((window as any).__PB_DEBUG_DRAG) {
            try {
              console.info('[rf][drag] commit', {
                id: change.id,
                x: snapped.x,
                y: snapped.y,
              });
            } catch {}
          }
        }
      });
    },
    [gridSize, setTilePosition, setTilePositionImmediate, state.tiles],
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
      addRelationship(edgeId, connection.source, connection.target, {
        sourceHandleId: connection.sourceHandle,
        targetHandleId: connection.targetHandle,
      });
      setEditingEdgeId(edgeId);
    },
    [addRelationship, state.tiles],
  );

  const handleNodeDragStart: NodeDragEventHandler = useCallback(
    (_event, node) => {
      const tile = state.tiles[node.id];
      if (!tile) return;
      // Avoid double bring-to-front churn: setActiveTile already elevates.
      setActiveTile(node.id);
      dragSnapshotRef.current = {
        tileId: node.id,
        originalPosition: { ...tile.position },
      };
      setDragCount((c) => c + 1);
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

  // We update live positions via onNodesChange (change.dragging),
  // so we avoid duplicating updates in onNodeDrag to reduce re-renders.

  const handleNodeDragStop: NodeDragEventHandler = useCallback(
    (_event, node) => {
      const tile = state.tiles[node.id];
      const snapshot = dragSnapshotRef.current;
      dragSnapshotRef.current = null;
      if (!tile || !snapshot || snapshot.tileId !== node.id) {
        setDragCount((c) => (c > 0 ? c - 1 : 0));
        return;
      }
      const snappedPosition = snapPointToGrid(node.position, gridSize);
      setTilePosition(node.id, snappedPosition);

      const delta = {
        x: snappedPosition.x - snapshot.originalPosition.x,
        y: snappedPosition.y - snapshot.originalPosition.y,
      };

      const bounds = readCanvasBounds();
      const canvasBounds = bounds ?? { width: 0, height: 0 };

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
      setDragCount((c) => (c > 0 ? c - 1 : 0));
    },
    [gridSize, onTileMove, privateBeachId, readCanvasBounds, reportTileMove, rewriteEnabled, setTilePosition, state.tiles],
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
      onNodePlacement({
        catalogId: payload.id,
        nodeType: payload.nodeType,
        size: { width: payload.defaultSize.width, height: payload.defaultSize.height },
        rawPosition: flowPosition,
        snappedPosition: snapped,
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
    const previous = previousRelationshipsRef.current;
    for (const relId of Object.keys(previous)) {
      if (!state.relationships[relId]) {
        const relationship = previous[relId];
        const traceId =
          relationship && state.tiles[relationship.sourceId]?.agentMeta?.trace?.enabled
            ? state.tiles[relationship.sourceId]?.agentMeta?.trace?.trace_id ?? null
            : null;
        void teardownRelationshipByData(relationship, traceId);
      }
    }
    previousRelationshipsRef.current = { ...state.relationships };
  }, [state.relationships, state.tiles, teardownRelationshipByData]);

  useEffect(() => {
    if (!traceOverlay) {
      return;
    }
    const relationship = state.relationships[traceOverlay.relationshipId];
    if (!relationship) {
      clearTraceLogs(traceOverlay.traceId);
      setTraceOverlay(null);
      return;
    }
    const sourceTile = state.tiles[relationship.sourceId];
    const nextTraceId =
      sourceTile && sourceTile.agentMeta?.trace?.enabled ? sourceTile.agentMeta?.trace?.trace_id ?? null : null;
    if (!nextTraceId) {
      clearTraceLogs(traceOverlay.traceId);
      setTraceOverlay(null);
      return;
    }
    if (nextTraceId !== traceOverlay.traceId) {
      setTraceOverlay((current) => (current ? { ...current, traceId: nextTraceId } : current));
    }
  }, [state.relationships, state.tiles, traceOverlay]);

  const handleRelationshipRetry = useCallback(
    ({ id }: { id: string }) => {
      delete relationshipSyncKeysRef.current[id];
      setRelationshipErrors((prev) => {
        if (!(id in prev)) {
          return prev;
        }
        const next = { ...prev };
        delete next[id];
        return next;
      });
      setSyncNonce((value) => value + 1);
    },
    [],
  );

  useEffect(() => {
    const presentIds = new Set(state.relationshipOrder);
    for (const cachedId of Object.keys(relationshipSyncKeysRef.current)) {
      if (!presentIds.has(cachedId)) {
        delete relationshipSyncKeysRef.current[cachedId];
      }
    }
    if (!privateBeachId) {
      return;
    }
    type PendingAssignment = {
      rel: RelationshipDescriptor;
      key: string;
      prompt: string;
      traceId: string | null;
      controller_session_id: string;
      child_session_id: string;
      cadence: ControllerUpdateCadence;
    };
    const seenPairs = new Set<string>();
    const pending: PendingAssignment[] = [];
    for (let index = state.relationshipOrder.length - 1; index >= 0; index -= 1) {
      const relId = state.relationshipOrder[index];
      const rel = state.relationships[relId];
      if (!rel) {
        continue;
      }
      const sourceTile = state.tiles[rel.sourceId];
      const targetTile = state.tiles[rel.targetId];
      if (!sourceTile || !targetTile) {
        continue;
      }
      const controllerSessionId = sourceTile.sessionMeta?.sessionId;
      const childSessionId = targetTile.sessionMeta?.sessionId;
      if (!controllerSessionId || !childSessionId) {
        continue;
      }
      const pairKey = `${controllerSessionId}|${childSessionId}`;
      if (seenPairs.has(pairKey)) {
        continue;
      }
      seenPairs.add(pairKey);
      const role = sourceTile.agentMeta?.role?.trim() ?? '';
      const responsibility = sourceTile.agentMeta?.responsibility?.trim() ?? '';
      const instructions = rel.instructions.trim();
      if (!role || !responsibility || !instructions) {
        continue;
      }
      const prompt = buildPromptTemplate(role, responsibility, instructions);
      const signature = [
        controllerSessionId,
        childSessionId,
        prompt,
        rel.updateMode,
        rel.pollFrequency,
      ].join('|');
      if (relationshipSyncKeysRef.current[relId] === signature) {
        continue;
      }
      const traceId =
        sourceTile.agentMeta?.trace?.enabled && sourceTile.agentMeta.trace.trace_id
          ? sourceTile.agentMeta.trace.trace_id
          : null;
      pending.push({
        rel,
        key: signature,
        prompt,
        traceId,
        controller_session_id: controllerSessionId,
        child_session_id: childSessionId,
        cadence: mapUpdateModeToCadence(rel.updateMode as UpdateMode),
      });
    }
    if (pending.length === 0) {
      return;
    }
    const orderedPending = pending.reverse();
    let cancelled = false;
    const run = async () => {
      const authToken = managerToken ?? (await refreshManagerToken());
      if (!authToken || cancelled) {
        return;
      }
      const uniqueControllers = Array.from(
        new Set(orderedPending.map((entry) => entry.controller_session_id)),
      );
      const leaseFailures = new Map<string, string>();
      for (const controllerId of uniqueControllers) {
        if (cancelled) {
          return;
        }
        try {
          await ensureControllerLease(controllerId, authToken);
        } catch (error) {
          const message =
            error instanceof Error ? error.message : 'Controller lease acquisition failed';
          leaseFailures.set(controllerId, message);
        }
      }
      if (leaseFailures.size > 0) {
        setRelationshipErrors((prev) => {
          const next = { ...prev };
          orderedPending.forEach((entry) => {
            const message = leaseFailures.get(entry.controller_session_id);
            if (message) {
              next[entry.rel.id] = message;
            }
          });
          return next;
        });
      }
      const runnableEntries = orderedPending.filter(
        (entry) => !leaseFailures.has(entry.controller_session_id),
      );
      if (runnableEntries.length === 0) {
        return;
      }
      const grouped = new Map<string, { traceId: string | null; entries: PendingAssignment[] }>();
      for (const entry of runnableEntries) {
        const key = entry.traceId ?? 'no-trace';
        if (!grouped.has(key)) {
          grouped.set(key, { traceId: entry.traceId, entries: [] });
        }
        grouped.get(key)!.entries.push(entry);
      }
      for (const group of grouped.values()) {
        if (cancelled) {
          break;
        }
        const relationshipIds = group.entries.map(({ rel }) => rel.id);
        if (group.traceId) {
          recordTraceLog(group.traceId, {
            source: 'assignments',
            level: 'info',
            message: `Syncing ${group.entries.length} assignment${group.entries.length === 1 ? '' : 's'}`,
            detail: {
              private_beach_id: privateBeachId,
              relationship_ids: relationshipIds,
            },
          });
        }
        try {
          const assignments = group.entries.map((entry) => ({
            controller_session_id: entry.controller_session_id,
            child_session_id: entry.child_session_id,
            prompt_template: entry.prompt,
            update_cadence: entry.cadence,
          }));
          const results = await batchControllerAssignments(
            privateBeachId,
            assignments,
            authToken,
            resolvedManagerUrl,
            group.traceId ?? undefined,
          );
          if (cancelled) {
            break;
          }
          const historyUpdates: Array<{ relId: string; entry: PairingHistoryEntry }> = [];
          results.forEach((result, index) => {
            const entry = group.entries[index];
            if (!entry) {
              return;
            }
            const relId = entry.rel.id;
            const timestamp = Date.now();
            const historyEntry: PairingHistoryEntry = {
              id: `${relId}-${timestamp}-${index}`,
              status: result?.ok ? 'ok' : 'error',
              message: result?.error ?? null,
              timestamp,
            };
            historyUpdates.push({ relId, entry: historyEntry });
            if (result?.ok) {
              relationshipSyncKeysRef.current[relId] = entry.key;
              setRelationshipErrors((prev) => {
                if (!(relId in prev)) {
                  return prev;
                }
                const next = { ...prev };
                delete next[relId];
                return next;
              });
              if (group.traceId) {
                recordTraceLog(group.traceId, {
                  source: 'assignments',
                  level: 'info',
                  message: 'Controller pairing synced',
                  detail: {
                    relationship_id: relId,
                    controller_session_id: entry.controller_session_id,
                    child_session_id: entry.child_session_id,
                  },
                });
              }
            } else {
              const message = result?.error || 'Failed to sync controller pairing';
              setRelationshipErrors((prev) => ({ ...prev, [relId]: message }));
              if (group.traceId) {
                recordTraceLog(group.traceId, {
                  source: 'assignments',
                  level: 'error',
                  message: 'Controller pairing failed',
                  detail: {
                    relationship_id: relId,
                    controller_session_id: entry.controller_session_id,
                    child_session_id: entry.child_session_id,
                    error: message,
                  },
                });
              }
            }
          });
          if (historyUpdates.length > 0) {
            setRelationshipSyncHistory((prev) => {
              const next = { ...prev };
              historyUpdates.forEach(({ relId, entry }) => {
                const existing = next[relId] ?? [];
                next[relId] = [entry, ...existing].slice(0, 8);
              });
              return next;
            });
          }
        } catch (error) {
          if (group.traceId) {
            recordTraceLog(group.traceId, {
              source: 'assignments',
              level: 'error',
              message: 'Failed to sync controller assignments',
              detail: {
                private_beach_id: privateBeachId,
                relationship_ids: relationshipIds,
                error: error instanceof Error ? error.message : String(error),
              },
            });
          }
          console.error('[rewrite-2] failed to sync controller assignments', error);
        }
      }
    };
    void run();
    return () => {
      cancelled = true;
    };
  }, [
    managerToken,
    ensureControllerLease,
    refreshManagerToken,
    privateBeachId,
    resolvedManagerUrl,
    state.relationshipOrder,
    state.relationships,
    state.tiles,
    syncNonce,
  ]);

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

  const edgeElements = useMemo(() =>
    state.relationshipOrder
      .map((relId) => {
        const rel = state.relationships[relId];
        if (!rel) return null;
        if (!state.tiles[rel.sourceId] || !state.tiles[rel.targetId]) {
          return null;
        }
        const sourceTile = state.tiles[rel.sourceId]!;
        const traceButtonEnabled = Boolean(sourceTile.agentMeta?.trace?.enabled && sourceTile.agentMeta?.trace?.trace_id);
        return {
          id: rel.id,
          type: 'assignment',
          source: rel.sourceId,
          target: rel.targetId,
          sourceHandle: rel.sourceHandleId ?? undefined,
          targetHandle: rel.targetHandleId ?? undefined,
          data: {
            instructions: rel.instructions,
            updateMode: rel.updateMode as UpdateMode,
            pollFrequency: rel.pollFrequency,
            isEditing: editingEdgeId === rel.id,
            status: relationshipErrors[rel.id] ? 'error' : 'ok',
            statusMessage: relationshipErrors[rel.id] ?? null,
            onRetry: relationshipErrors[rel.id] ? handleRelationshipRetry : undefined,
            onSave: handleEdgeSave,
            onEdit: handleEdgeEdit,
            onDelete: handleEdgeDelete,
            onShowTrace: traceButtonEnabled ? handleShowTraceOverlay : undefined,
          },
        } satisfies Edge<AssignmentEdgeData>;
      })
      .filter((edge): edge is Edge<AssignmentEdgeData> => Boolean(edge)),
  [editingEdgeId, handleEdgeDelete, handleEdgeEdit, handleEdgeSave, handleRelationshipRetry, handleShowTraceOverlay, relationshipErrors, state.relationshipOrder, state.relationships, state.tiles]);

  const traceOverlayProps = useMemo(() => {
    if (!traceOverlay) {
      return null;
    }
    const relationship = state.relationships[traceOverlay.relationshipId];
    if (!relationship) {
      return null;
    }
    const sourceTile = state.tiles[relationship.sourceId];
    const targetTile = state.tiles[relationship.targetId];
    if (!sourceTile || !sourceTile.agentMeta?.trace?.enabled || !sourceTile.agentMeta?.trace?.trace_id) {
      return null;
    }
    const history = relationshipSyncHistory[relationship.id] ?? [];
    return {
      traceId: traceOverlay.traceId,
      agentRole: sourceTile.agentMeta.role,
      agentResponsibility: sourceTile.agentMeta.responsibility,
      instructions: traceOverlay.instruction,
      cadence: traceOverlay.cadence,
      pollFrequency: traceOverlay.pollFrequency,
      sourceSessionId: relationship.sourceSessionId ?? null,
      targetSessionId: relationship.targetSessionId ?? null,
      pairingHistory: history,
      logs: traceLogs,
    };
  }, [relationshipSyncHistory, state.relationships, state.tiles, traceLogs, traceOverlay]);

  const isDragging = dragCount > 0;
  const zoomOnScrollEnabled = !interactiveId;

  // Canvas wrapper uses theme background tokens (light: soft neutral, dark: deep neutral).
  // Keep backdrop blur off during drag to avoid overdraw.
  return (
    <CanvasDragStateProvider value={isDragging}>
      <div
        ref={wrapperRef}
        className={`relative flex-1 h-full w-full overflow-hidden bg-background ${isDragging ? '' : 'backdrop-blur-2xl'}`}
        data-testid="flow-canvas"
      >
        <ReactFlow
          nodes={nodes}
          edges={edgeElements}
          nodeTypes={memoizedNodeTypes}
          edgeTypes={memoizedEdgeTypes}
          defaultEdgeOptions={defaultEdgeOptions}
          // Restrict dragging to the tile header to avoid drag conflicts
          // with dynamic content inside the node body (terminal, etc.).
          nodeDragHandle=".rf-drag-handle"
          onNodesChange={handleNodesChange}
          onEdgesChange={() => undefined}
          onConnect={handleConnect}
          onNodeDragStart={handleNodeDragStart}
        onNodeDragStop={handleNodeDragStop}
          onMoveEnd={handleViewportMoveEnd}
          connectionMode="loose"
          connectionRadius={36}
          // Always allow node dragging; we restrict the drag start area via
          // `nodeDragHandle` so interactive content does not interfere.
          nodesDraggable
          nodesConnectable
          elementsSelectable={false}
          // Keep nodes rendered during drag to avoid viewport-culling flicker
          onlyRenderVisibleElements={false}
          selectNodesOnDrag={false}
          panOnScroll={false}
          // Allow panning with left-drag on the pane (not on nodes)
          panOnDrag
          // Enable wheel-driven zoom unless a tile is in interactive mode so scrolling stays scoped to that tile.
          zoomOnScroll={zoomOnScrollEnabled}
          zoomOnPinch
          zoomOnDoubleClick={false}
          fitView={false}
          elevateNodesOnSelect={false}
          proOptions={{ hideAttribution: true }}
          className="h-full w-full"
          minZoom={0.2}
          maxZoom={1.75}
          style={{ width: '100%', height: '100%', transform: 'translateZ(0)', willChange: 'transform' }}
        >
          {/** Subtle blue-gray grid that reads well on both themes */}
          <Background gap={gridSize} color="rgba(124, 144, 171, 0.14)" />
        </ReactFlow>
        <CanvasViewportControls />
        {traceOverlayProps ? (
          <TraceMonitorPanel
            traceId={traceOverlayProps.traceId}
            agentRole={traceOverlayProps.agentRole}
            agentResponsibility={traceOverlayProps.agentResponsibility}
            instructions={traceOverlayProps.instructions}
            cadence={traceOverlayProps.cadence}
            pollFrequency={traceOverlayProps.pollFrequency}
            sourceSessionId={traceOverlayProps.sourceSessionId}
            targetSessionId={traceOverlayProps.targetSessionId}
            pairingHistory={traceOverlayProps.pairingHistory}
            logs={traceOverlayProps.logs}
            onClose={() => {
              clearTraceLogs(traceOverlayProps.traceId);
              setTraceOverlay(null);
            }}
          />
        ) : null}
      </div>
    </CanvasDragStateProvider>
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

function mapUpdateModeToCadence(mode: UpdateMode): ControllerUpdateCadence {
  if (mode === 'push') {
    return 'fast';
  }
  return 'slow';
}

function buildPromptTemplate(role: string, responsibility: string, instructions: string): string {
  const parts = [
    `Role:\n${role.trim()}`,
    `Responsibility:\n${responsibility.trim()}`,
    `Instructions:\n${instructions.trim()}`,
  ];
  return parts.join('\n\n');
}

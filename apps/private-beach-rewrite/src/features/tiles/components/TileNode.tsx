'use client';

import { useCallback, useMemo, useRef } from 'react';
import type { MouseEvent, PointerEvent } from 'react';
import { ApplicationTile } from '@/components/ApplicationTile';
import { TILE_HEADER_HEIGHT, TILE_GRID_SNAP_PX } from '../constants';
import { useTileActions } from '../store';
import type { TileDescriptor, TilePosition, TileSessionMeta, TileSize } from '../types';
import { snapPosition, snapSize } from '../utils';
import { emitTelemetry } from '../../../../../private-beach/src/lib/telemetry';
import { useCanvasEvents } from '@/features/canvas/CanvasEventsContext';
import type { TileMovePayload } from '@/features/canvas/types';

type TileNodeProps = {
  tile: TileDescriptor;
  orderIndex: number;
  isActive: boolean;
  isResizing: boolean;
  privateBeachId: string;
  managerUrl: string;
  rewriteEnabled: boolean;
  onMove?: (payload: TileMovePayload) => void;
};

type ResizeState = {
  pointerId: number;
  startX: number;
  startY: number;
  width: number;
  height: number;
  lastSize?: TileSize;
};

type DragState = {
  pointerId: number;
  startX: number;
  startY: number;
  initialPosition: TilePosition;
  lastPosition: TilePosition;
  hasStarted: boolean;
};

function metaEqual(a: TileSessionMeta | null | undefined, b: TileSessionMeta | null | undefined) {
  if (!a && !b) return true;
  if (!a || !b) return false;
  return (
    a.sessionId === b.sessionId &&
    a.title === b.title &&
    a.status === b.status &&
    a.harnessType === b.harnessType &&
    (a.pendingActions ?? null) === (b.pendingActions ?? null)
  );
}

function isInteractiveElement(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  if (target.closest('[data-tile-drag-ignore="true"]')) {
    return true;
  }
  return Boolean(target.closest('button, input, textarea, select, a, label'));
}

export function TileNode({
  tile,
  orderIndex,
  isActive,
  isResizing,
  privateBeachId,
  managerUrl,
  rewriteEnabled,
  onMove,
}: TileNodeProps) {
  const { removeTile, bringToFront, setActiveTile, beginResize, resizeTile, endResize, updateTileMeta, setTilePosition } =
    useTileActions();
  const resizeStateRef = useRef<ResizeState | null>(null);
  const dragStateRef = useRef<DragState | null>(null);
  const { reportTileMove } = useCanvasEvents();

  const zIndex = useMemo(() => 10 + orderIndex, [orderIndex]);

  const handleRemove = useCallback(
    (event: MouseEvent<HTMLButtonElement>) => {
      event.stopPropagation();
      emitTelemetry('canvas.tile.remove', {
        privateBeachId,
        tileId: tile.id,
        rewriteEnabled,
      });
      removeTile(tile.id);
    },
    [privateBeachId, removeTile, rewriteEnabled, tile.id],
  );

  const handlePointerDown = useCallback(
    (event: PointerEvent<HTMLElement>) => {
      if (event.button !== 0) {
        return;
      }
      bringToFront(tile.id);
      setActiveTile(tile.id);
      if (isInteractiveElement(event.target)) {
        dragStateRef.current = null;
        return;
      }
      const state: DragState = {
        pointerId: event.pointerId,
        startX: event.clientX,
        startY: event.clientY,
        initialPosition: { ...tile.position },
        lastPosition: { ...tile.position },
        hasStarted: false,
      };
      dragStateRef.current = state;
      try {
        event.currentTarget.setPointerCapture(event.pointerId);
      } catch {
        // ignore pointer capture failures
      }
    },
    [bringToFront, setActiveTile, tile.id, tile.position],
  );

  const handlePointerMove = useCallback(
    (event: PointerEvent<HTMLElement>) => {
      const state = dragStateRef.current;
      if (!state || state.pointerId !== event.pointerId) {
        return;
      }
      const deltaX = event.clientX - state.startX;
      const deltaY = event.clientY - state.startY;
      if (!state.hasStarted) {
        if (Math.abs(deltaX) + Math.abs(deltaY) < 4) {
          return;
        }
        state.hasStarted = true;
        emitTelemetry('canvas.drag.start', {
          privateBeachId,
          tileId: tile.id,
          nodeType: tile.nodeType,
          x: state.initialPosition.x,
          y: state.initialPosition.y,
          rewriteEnabled,
        });
      }
      const nextPosition = snapPosition({
        x: state.initialPosition.x + deltaX,
        y: state.initialPosition.y + deltaY,
      });
      if (nextPosition.x === state.lastPosition.x && nextPosition.y === state.lastPosition.y) {
        return;
      }
      state.lastPosition = nextPosition;
      setTilePosition(tile.id, nextPosition);
      event.preventDefault();
    },
    [privateBeachId, rewriteEnabled, setTilePosition, tile.id, tile.nodeType],
  );

  const releaseDragPointer = useCallback((event: PointerEvent<HTMLElement>) => {
    try {
      if (event.currentTarget.hasPointerCapture(event.pointerId)) {
        event.currentTarget.releasePointerCapture(event.pointerId);
      }
    } catch {
      // ignore capture release failures
    }
  }, []);

  const handlePointerUp = useCallback(
    (event: PointerEvent<HTMLElement>) => {
      const state = dragStateRef.current;
      if (state && state.pointerId === event.pointerId) {
        if (state.hasStarted) {
          const surfaceElement = event.currentTarget.closest('.tile-canvas__surface') as HTMLElement | null;
          const boundsRect = surfaceElement?.getBoundingClientRect();
          const canvasBounds = boundsRect
            ? { width: boundsRect.width, height: boundsRect.height }
            : { width: 0, height: 0 };
          const rawPosition = {
            x: state.initialPosition.x + (event.clientX - state.startX),
            y: state.initialPosition.y + (event.clientY - state.startY),
          };
          const snappedPosition = state.lastPosition ?? state.initialPosition;
          const payload: TileMovePayload = {
            tileId: tile.id,
            source: 'pointer',
            rawPosition,
            snappedPosition,
            delta: {
              x: snappedPosition.x - state.initialPosition.x,
              y: snappedPosition.y - state.initialPosition.y,
            },
            canvasBounds,
            gridSize: TILE_GRID_SNAP_PX,
            timestamp: Date.now(),
          };
          onMove?.(payload);
          reportTileMove({
            tileId: tile.id,
            size: { ...tile.size },
            originalPosition: state.initialPosition,
            rawPosition,
            snappedPosition,
            source: 'pointer',
          });
          emitTelemetry('canvas.drag.stop', {
            privateBeachId,
            tileId: tile.id,
            nodeType: tile.nodeType,
            x: snappedPosition.x,
            y: snappedPosition.y,
            rewriteEnabled,
          });
          console.info('[ws-d] tile moved', {
            privateBeachId,
            tileId: tile.id,
            position: { ...snappedPosition },
            rewriteEnabled,
          });
        }
        dragStateRef.current = null;
      }
      releaseDragPointer(event);
    },
    [onMove, privateBeachId, releaseDragPointer, reportTileMove, rewriteEnabled, tile.id, tile.nodeType, tile.size],
  );

  const handlePointerCancel = useCallback(
    (event: PointerEvent<HTMLElement>) => {
      dragStateRef.current = null;
      releaseDragPointer(event);
    },
    [releaseDragPointer],
  );

  const handleResizePointerDown = useCallback(
    (event: PointerEvent<HTMLButtonElement>) => {
      event.preventDefault();
      event.stopPropagation();
      bringToFront(tile.id);
      setActiveTile(tile.id);
      beginResize(tile.id);
      const { width, height } = tile.size;
      resizeStateRef.current = {
        pointerId: event.pointerId,
        startX: event.clientX,
        startY: event.clientY,
        width,
        height,
        lastSize: { width, height },
      };
      event.currentTarget.setPointerCapture(event.pointerId);
    },
    [beginResize, bringToFront, setActiveTile, tile.id, tile.size],
  );

  const handleResizePointerMove = useCallback(
    (event: PointerEvent<HTMLButtonElement>) => {
      const state = resizeStateRef.current;
      if (!state || state.pointerId !== event.pointerId) {
        return;
      }
      const deltaX = event.clientX - state.startX;
      const deltaY = event.clientY - state.startY;
      const nextSize = snapSize({
        width: state.width + deltaX,
        height: state.height + deltaY,
      });
      state.lastSize = nextSize;
      resizeTile(tile.id, nextSize);
    },
    [resizeTile, tile.id],
  );

  const releaseResizePointer = useCallback((event: PointerEvent<HTMLButtonElement>) => {
    try {
      if (event.currentTarget.hasPointerCapture(event.pointerId)) {
        event.currentTarget.releasePointerCapture(event.pointerId);
      }
    } catch {
      // Ignore release errors
    }
  }, []);

  const handleResizePointerUp = useCallback(
    (event: PointerEvent<HTMLButtonElement>) => {
      const state = resizeStateRef.current;
      if (!state || state.pointerId !== event.pointerId) {
        return;
      }
      releaseResizePointer(event);
      endResize(tile.id);
      if (state.lastSize) {
        emitTelemetry('canvas.resize.stop', {
          privateBeachId,
          tileId: tile.id,
          width: state.lastSize.width,
          height: state.lastSize.height,
          rewriteEnabled,
        });
        console.info('[ws-d] tile resized', {
          privateBeachId,
          tileId: tile.id,
          size: { ...state.lastSize },
          rewriteEnabled,
        });
      }
      resizeStateRef.current = null;
    },
    [endResize, privateBeachId, releaseResizePointer, rewriteEnabled, tile.id],
  );

  const handleResizePointerCancel = useCallback(
    (event: PointerEvent<HTMLButtonElement>) => {
      releaseResizePointer(event);
      endResize(tile.id);
      resizeStateRef.current = null;
    },
    [endResize, releaseResizePointer, tile.id],
  );

  const title = tile.sessionMeta?.title ?? tile.sessionMeta?.sessionId ?? 'Application Tile';
  const subtitle = useMemo(() => {
    if (!tile.sessionMeta) return 'Disconnected';
    if (tile.sessionMeta.status) return tile.sessionMeta.status;
    if (tile.sessionMeta.harnessType) return tile.sessionMeta.harnessType;
    return 'Attached';
  }, [tile.sessionMeta]);

  const handleMetaChange = useCallback(
    (meta: TileSessionMeta | null) => {
      const current = tile.sessionMeta ?? null;
      if (metaEqual(current, meta)) {
        return;
      }
      updateTileMeta(tile.id, meta);
    },
    [tile.id, tile.sessionMeta, updateTileMeta],
  );

  return (
    <article
      className={`tile-node${isActive ? ' tile-node--active' : ''}${isResizing ? ' tile-node--resizing' : ''}`}
      style={{
        left: `${tile.position.x}px`,
        top: `${tile.position.y}px`,
        width: `${tile.size.width}px`,
        height: `${tile.size.height}px`,
        zIndex,
      }}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerUp}
      onPointerCancel={handlePointerCancel}
      data-tile-id={tile.id}
    >
      <header className="tile-node__header" style={{ minHeight: TILE_HEADER_HEIGHT }}>
        <div className="tile-node__title">
          <span title={title}>{title}</span>
          {subtitle ? <small>{subtitle}</small> : null}
        </div>
        <button type="button" className="tile-node__remove" onClick={handleRemove} data-tile-drag-ignore="true">
          ×
        </button>
      </header>
      <section className="tile-node__body" data-tile-drag-ignore="true">
        <ApplicationTile
          tileId={tile.id}
          privateBeachId={privateBeachId}
          managerUrl={managerUrl}
          sessionMeta={tile.sessionMeta ?? null}
          onSessionMetaChange={handleMetaChange}
        />
      </section>
      <button
        type="button"
        className="tile-node__resize tile-node__resize--se"
        aria-label="Resize tile"
        onPointerDown={handleResizePointerDown}
        onPointerMove={handleResizePointerMove}
        onPointerUp={handleResizePointerUp}
        onPointerCancel={handleResizePointerCancel}
        data-tile-drag-ignore="true"
      />
    </article>
  );
}

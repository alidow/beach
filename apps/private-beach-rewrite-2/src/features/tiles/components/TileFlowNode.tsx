'use client';

import { useCallback, useMemo, useRef } from 'react';
import type { MouseEvent, PointerEvent } from 'react';
import type { NodeProps } from 'reactflow';
import { ApplicationTile } from '@/components/ApplicationTile';
import { cn } from '@/lib/cn';
import { TILE_HEADER_HEIGHT } from '../constants';
import { useTileActions } from '../store';
import type { TileDescriptor, TileSessionMeta } from '../types';
import { snapSize } from '../utils';
import { emitTelemetry } from '../../../../../private-beach/src/lib/telemetry';

type TileFlowNodeData = {
  tile: TileDescriptor;
  orderIndex: number;
  isActive: boolean;
  isResizing: boolean;
  privateBeachId: string;
  managerUrl: string;
  rewriteEnabled: boolean;
};

type ResizeState = {
  pointerId: number;
  startX: number;
  startY: number;
  width: number;
  height: number;
  lastSize?: { width: number; height: number };
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

type Props = NodeProps<TileFlowNodeData>;

export function TileFlowNode({ data }: Props) {
  const { tile, orderIndex, isActive, isResizing, privateBeachId, managerUrl, rewriteEnabled } = data;
  const { removeTile, bringToFront, setActiveTile, beginResize, resizeTile, endResize, updateTileMeta } = useTileActions();
  const resizeStateRef = useRef<ResizeState | null>(null);

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
        event.stopPropagation();
      }
    },
    [bringToFront, setActiveTile, tile.id],
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
      try {
        event.currentTarget.setPointerCapture(event.pointerId);
      } catch {
        // ignore pointer capture issues
      }
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
      // ignore release errors
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

  const nodeClass = cn(
    'group relative flex h-full w-full select-none flex-col overflow-hidden rounded-2xl border border-slate-700/60 bg-slate-950/80 text-slate-200 shadow-[0_28px_80px_rgba(2,6,23,0.6)] backdrop-blur-xl transition-all duration-200',
    isActive && 'border-sky-400/60 shadow-[0_32px_90px_rgba(14,165,233,0.35)]',
    isResizing && 'cursor-[se-resize]',
  );

  return (
    <article
      className={nodeClass}
      style={{ width: '100%', height: '100%', zIndex }}
      data-testid={`rf__node-tile:${tile.id}`}
      data-tile-id={tile.id}
      onPointerDown={handlePointerDown}
    >
      <header
        className="flex min-h-[44px] items-center justify-between border-b border-white/10 bg-slate-900/80 px-4 py-2.5 backdrop-blur"
        style={{ minHeight: TILE_HEADER_HEIGHT }}
      >
        <div className="flex min-w-0 flex-col gap-1">
          <span className="truncate text-sm font-semibold text-white/90" title={title}>
            {title}
          </span>
          {subtitle ? <small className="truncate text-[11px] uppercase tracking-[0.18em] text-slate-400">{subtitle}</small> : null}
        </div>
        <button
          type="button"
          className="inline-flex h-7 w-7 items-center justify-center rounded-full border border-red-500/40 bg-red-500/15 text-base font-semibold text-red-200 transition hover:border-red-400/70 hover:bg-red-500/25 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-400/60"
          onClick={handleRemove}
          data-tile-drag-ignore="true"
        >
          ×
        </button>
      </header>
      <section
        className="flex flex-1 flex-col gap-4 overflow-hidden bg-slate-950/60 p-4"
        data-tile-drag-ignore="true"
      >
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
        className="absolute bottom-3 right-3 z-10 h-5 w-5 cursor-nwse-resize rounded-md border border-sky-400/40 bg-[radial-gradient(circle_at_top_left,rgba(56,189,248,0.6),rgba(56,189,248,0.05))] text-transparent transition hover:border-sky-400/60 hover:shadow-[0_0_12px_rgba(56,189,248,0.45)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-400/60"
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

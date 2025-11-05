'use client';

import { useCallback, useMemo } from 'react';
import type { MouseEvent, PointerEvent } from 'react';
import { NodeResizer, type NodeProps } from 'reactflow';
import { ApplicationTile } from '@/components/ApplicationTile';
import { TILE_HEADER_HEIGHT, MIN_TILE_WIDTH, MIN_TILE_HEIGHT } from '../constants';
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

  const handleResizeStart = useCallback(() => {
    beginResize(tile.id);
    bringToFront(tile.id);
    setActiveTile(tile.id);
  }, [beginResize, bringToFront, setActiveTile, tile.id]);

  const handleResize = useCallback((_: unknown, params: { width: number; height: number }) => {
    const snapped = snapSize({ width: params.width, height: params.height });
    resizeTile(tile.id, snapped);
  }, [resizeTile, tile.id]);

  const handleResizeEnd = useCallback((_: unknown, params: { width: number; height: number }) => {
    const snapped = snapSize({ width: params.width, height: params.height });
    resizeTile(tile.id, snapped);
    endResize(tile.id);
    emitTelemetry('canvas.resize.stop', {
      privateBeachId,
      tileId: tile.id,
      width: snapped.width,
      height: snapped.height,
      rewriteEnabled,
    });
  }, [endResize, privateBeachId, resizeTile, rewriteEnabled, tile.id]);

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
        width: '100%',
        height: '100%',
        zIndex,
      }}
      data-testid={`rf__node-tile:${tile.id}`}
      data-tile-id={tile.id}
      onPointerDown={handlePointerDown}
    >
      <NodeResizer
        isVisible={isActive || isResizing}
        minWidth={MIN_TILE_WIDTH}
        minHeight={MIN_TILE_HEIGHT}
        onResizeStart={handleResizeStart}
        onResize={handleResize}
        onResizeEnd={handleResizeEnd}
        handleStyle={{ width: 14, height: 14, borderRadius: 4, background: 'rgba(59,130,246,0.7)', border: '1px solid rgba(15,23,42,0.6)' }}
        lineStyle={{ borderColor: 'rgba(59,130,246,0.35)' }}
      />
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
    </article>
  );
}

'use client';

import { useCallback, useEffect, useMemo, useRef } from 'react';
import type { MouseEvent, PointerEvent } from 'react';
import { NodeResizer, type NodeProps, useStore, type ReactFlowState } from 'reactflow';
import { ApplicationTile } from '@/components/ApplicationTile';
import { TILE_HEADER_HEIGHT, MIN_TILE_WIDTH, MIN_TILE_HEIGHT } from '../constants';
import { useTileActions } from '../store';
import type { TileDescriptor, TileSessionMeta, TileViewportSnapshot } from '../types';
import { snapSize } from '../utils';
import { emitTelemetry } from '../../../../../private-beach/src/lib/telemetry';
import { computeAutoResizeSize } from '../autoResize';

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

const zoomSelector = (state: ReactFlowState) => state.transform[2] ?? 1;
const AUTO_RESIZE_TOLERANCE_PX = 1;

function logAutoResizeEvent(tileId: string, step: string, detail: Record<string, unknown> = {}) {
  if (typeof window === 'undefined') {
    return;
  }
  try {
    console.info('[tile][auto-resize]', step, JSON.stringify({ tileId, ...detail }));
  } catch {
    console.info('[tile][auto-resize]', step, { tileId, ...detail });
  }
}

type Props = NodeProps<TileFlowNodeData>;

export function TileFlowNode({ data, dragging }: Props) {
  const { tile, orderIndex, isActive, isResizing, privateBeachId, managerUrl, rewriteEnabled } = data;
  const { removeTile, bringToFront, setActiveTile, beginResize, resizeTile, endResize, updateTileMeta } = useTileActions();
  const nodeRef = useRef<HTMLElement | null>(null);
  const viewportMetricsRef = useRef<TileViewportSnapshot | null>(null);
  const zoom = useStore(zoomSelector);

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

  const handleAutoResize = useCallback(() => {
    if (isResizing) {
      logAutoResizeEvent(tile.id, 'skip-resizing');
      return;
    }
    const viewportMetrics = viewportMetricsRef.current;
    logAutoResizeEvent(tile.id, 'attempt', {
      hasMetrics: Boolean(viewportMetrics),
      zoom: zoom ?? null,
    });
    if (!viewportMetrics) {
      logAutoResizeEvent(tile.id, 'missing-metrics');
      return;
    }
    const hostRows = viewportMetrics.hostRows ?? null;
    const hostCols = viewportMetrics.hostCols ?? null;
    const pixelsPerRow = viewportMetrics.pixelsPerRow ?? null;
    const pixelsPerCol = viewportMetrics.pixelsPerCol ?? null;
    if (!hostRows || !hostCols || !pixelsPerRow || !pixelsPerCol) {
      logAutoResizeEvent(tile.id, 'incomplete-metrics', {
        hostRows,
        hostCols,
        pixelsPerRow,
        pixelsPerCol,
      });
      return;
    }
    const container = nodeRef.current;
    if (!container) {
      logAutoResizeEvent(tile.id, 'missing-container');
      return;
    }
    const terminalRoot = container.querySelector<HTMLElement>(
      `[data-terminal-root="true"][data-terminal-tile="${tile.id}"]`,
    );
    const terminalContent =
      terminalRoot?.querySelector<HTMLElement>('[data-terminal-content="true"]') ?? terminalRoot;
    const terminal = terminalContent ?? terminalRoot;
    if (!terminal) {
      logAutoResizeEvent(tile.id, 'missing-terminal');
      return;
    }
    const tileRect = container.getBoundingClientRect();
    const terminalRect = terminal.getBoundingClientRect();
    if (tileRect.width <= 0 || tileRect.height <= 0 || terminalRect.width <= 0 || terminalRect.height <= 0) {
      logAutoResizeEvent(tile.id, 'invalid-rect', { tileRect, terminalRect });
      return;
    }
    const zoomFactor = zoom && zoom > 0 ? zoom : 1;
    const tileWidthPx = tileRect.width / zoomFactor;
    const tileHeightPx = tileRect.height / zoomFactor;
    const terminalWidthPx = terminalRect.width / zoomFactor;
    const terminalHeightPx = terminalRect.height / zoomFactor;
    const chromeWidthPx = Math.max(0, tileWidthPx - terminalWidthPx);
    const chromeHeightPx = Math.max(0, tileHeightPx - terminalHeightPx);
    const viewportCols = viewportMetrics.viewportCols ?? hostCols ?? null;
    const viewportRows = viewportMetrics.viewportRows ?? hostRows ?? null;
    const observedCellWidth =
      viewportCols && viewportCols > 0 ? terminalWidthPx / viewportCols : null;
    const observedRowHeight =
      viewportRows && viewportRows > 0 ? terminalHeightPx / viewportRows : null;
    const nextSize = computeAutoResizeSize({
      metrics: viewportMetrics,
      chromeWidthPx,
      chromeHeightPx,
      zoom: zoom ?? 1,
      observedCellWidth,
      observedRowHeight,
    });
    if (!nextSize) {
      logAutoResizeEvent(tile.id, 'compute-failed', { chromeWidthPx, chromeHeightPx });
      return;
    }
    if (nextSize.width === tile.size.width && nextSize.height === tile.size.height) {
      logAutoResizeEvent(tile.id, 'no-op', nextSize);
      return;
    }
    const deltaWidth = Math.abs(nextSize.width - tile.size.width);
    const deltaHeight = Math.abs(nextSize.height - tile.size.height);
    if (deltaWidth <= AUTO_RESIZE_TOLERANCE_PX && deltaHeight <= AUTO_RESIZE_TOLERANCE_PX) {
      logAutoResizeEvent(tile.id, 'tolerance-skip', {
        size: nextSize,
        current: tile.size,
      });
      return;
    }
    logAutoResizeEvent(tile.id, 'apply', {
      size: nextSize,
      chromeWidthPx,
      chromeHeightPx,
      zoom: zoom ?? 1,
      observedCellWidth,
      observedRowHeight,
    });
    resizeTile(tile.id, nextSize);
    emitTelemetry('canvas.resize.auto', {
      privateBeachId,
      tileId: tile.id,
      hostRows,
      hostCols,
      viewportRows: viewportMetrics.viewportRows ?? null,
      viewportCols: viewportMetrics.viewportCols ?? null,
      pixelsPerRow,
      pixelsPerCol,
      zoom: zoom ?? 1,
      rewriteEnabled,
      size: nextSize,
    });
  }, [
    isResizing,
    nodeRef,
    privateBeachId,
    resizeTile,
    rewriteEnabled,
    tile.id,
    tile.size,
    zoom,
  ]);

  const handleViewportMetricsChange = useCallback(
    (snapshot: TileViewportSnapshot | null) => {
      viewportMetricsRef.current = snapshot;
      if (!snapshot) {
        logAutoResizeEvent(tile.id, 'metrics-cleared');
        return;
      }
      logAutoResizeEvent(tile.id, 'metrics-updated', {
        hostRows: snapshot.hostRows,
        hostCols: snapshot.hostCols,
        viewportRows: snapshot.viewportRows,
        viewportCols: snapshot.viewportCols,
        pixelsPerRow: snapshot.pixelsPerRow,
        pixelsPerCol: snapshot.pixelsPerCol,
      });
    },
    [tile.id],
  );

  useEffect(() => {
    if (!tile.sessionMeta?.sessionId) {
      viewportMetricsRef.current = null;
      logAutoResizeEvent(tile.id, 'session-cleared');
    }
  }, [tile.id, tile.sessionMeta?.sessionId]);

  return (
    <article
      ref={nodeRef}
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
          onViewportMetricsChange={handleViewportMetricsChange}
          interactive={tile.interactive !== false}
        />
      </section>
    </article>
  );
}

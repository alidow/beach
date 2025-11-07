'use client';

import { useCallback, useEffect, useRef } from 'react';
import type { TerminalViewerState } from '../../../private-beach/src/hooks/terminalViewerTypes';
import { BeachTerminal, type TerminalViewportState } from '../../../beach-surfer/src/components/BeachTerminal';
import { rewriteTerminalSizingStrategy } from './rewriteTerminalSizing';
import { cn } from '@/lib/cn';
import type { TileViewportSnapshot } from '@/features/tiles';

type SessionViewerProps = {
  viewer: TerminalViewerState;
  tileId: string;
  className?: string;
  sessionId?: string | null;
  disableViewportMeasurements?: boolean;
  onViewportMetrics?: (snapshot: TileViewportSnapshot | null) => void;
};

function normalizeMetric(value: number | null | undefined): number | null {
  if (typeof value !== 'number') {
    return null;
  }
  if (!Number.isFinite(value) || value <= 0) {
    return null;
  }
  return value;
}

export function SessionViewer({
  viewer,
  tileId,
  className,
  sessionId,
  disableViewportMeasurements = false,
  onViewportMetrics,
}: SessionViewerProps) {
  const status = viewer.status ?? 'idle';
  const showLoading = status === 'idle' || status === 'connecting' || status === 'reconnecting';
  const showError = status === 'error' && Boolean(viewer.error);
  const metricsRef = useRef<TileViewportSnapshot | null>(null);

  useEffect(() => {
    if (typeof window === 'undefined') {
      return undefined;
    }
    const store = viewer.store;
    if (!store || typeof store.subscribe !== 'function' || typeof store.getSnapshot !== 'function') {
      return undefined;
    }
    const logSnapshot = (reason: string) => {
      try {
        const snap = store.getSnapshot();
        const payload = snap
          ? {
              sessionId,
              reason,
              rows: snap.rows.length,
              viewportHeight: snap.viewportHeight,
              baseRow: snap.baseRow,
              followTail: snap.followTail,
            }
          : { sessionId, reason, rows: null };
        // eslint-disable-next-line no-console
        console.info('[rewrite-terminal][store]', JSON.stringify(payload));
      } catch (error) {
        // eslint-disable-next-line no-console
        console.warn('[rewrite-terminal][store] error reading snapshot', error);
      }
    };
    logSnapshot('initial');
    const unsubscribe = store.subscribe(() => logSnapshot('update'));
    return () => {
      try {
        unsubscribe();
      } catch (error) {
        // eslint-disable-next-line no-console
        console.warn('[rewrite-terminal][store] unsubscribe error', error);
      }
    };
  }, [sessionId, viewer.store]);

  useEffect(() => {
    if (typeof window === 'undefined') {
      return;
    }
    let snapshotSummary: { rows: number; viewportHeight: number; baseRow: number } | null = null;
    try {
      const gridSnapshot = viewer.store?.getSnapshot?.();
      if (gridSnapshot) {
        snapshotSummary = {
          rows: gridSnapshot.rows.length,
          viewportHeight: gridSnapshot.viewportHeight,
          baseRow: gridSnapshot.baseRow,
        };
      }
    } catch (error) {
      snapshotSummary = { rows: -1, viewportHeight: -1, baseRow: -1 };
      // eslint-disable-next-line no-console
      console.warn('[rewrite-terminal][ui] error reading grid snapshot', error);
    }
    const payload = {
      sessionId,
      status,
      showLoading,
      showError,
      hasTransport: Boolean(viewer.transport),
      transportVersion: viewer.transportVersion ?? 0,
      hasStore: Boolean(viewer.store),
      snapshot: snapshotSummary,
      latencyMs: viewer.latencyMs ?? null,
      error: viewer.error ?? null,
    };
    // eslint-disable-next-line no-console
    console.info('[rewrite-terminal][ui]', JSON.stringify(payload));
  }, [showError, showLoading, status, viewer.error, viewer.latencyMs, viewer.store, viewer.transport, viewer.transportVersion, sessionId]);

  useEffect(() => {
    metricsRef.current = null;
    onViewportMetrics?.(null);
    return () => {
      onViewportMetrics?.(null);
    };
  }, [onViewportMetrics, sessionId, tileId]);

  const handleViewportStateChange = useCallback(
    (state: TerminalViewportState) => {
      if (!onViewportMetrics) {
        return;
      }
      const snapshot: TileViewportSnapshot = {
        tileId,
        hostRows: normalizeMetric(state.hostViewportRows),
        hostCols: normalizeMetric(state.hostCols),
        viewportRows: normalizeMetric(state.viewportRows),
        viewportCols: normalizeMetric(state.viewportCols),
        pixelsPerRow: normalizeMetric(state.pixelsPerRow),
        pixelsPerCol: normalizeMetric(state.pixelsPerCol),
      };
      const previous = metricsRef.current;
      if (
        previous &&
        previous.hostRows === snapshot.hostRows &&
        previous.hostCols === snapshot.hostCols &&
        previous.viewportRows === snapshot.viewportRows &&
        previous.viewportCols === snapshot.viewportCols &&
        previous.pixelsPerRow === snapshot.pixelsPerRow &&
        previous.pixelsPerCol === snapshot.pixelsPerCol
      ) {
        return;
      }
      metricsRef.current = snapshot;
      onViewportMetrics(snapshot);
    },
    [onViewportMetrics, tileId],
  );

  return (
    <div className={cn('relative flex h-full min-h-0 w-full flex-1 overflow-hidden', className)} data-status={status}>
      <div
        className="flex h-full w-full flex-1"
        data-terminal-root="true"
        data-terminal-tile={tileId}
      >
        <BeachTerminal
          className="flex h-full w-full flex-1 border border-slate-800/70 bg-[#060910]/95 shadow-[0_30px_80px_rgba(8,12,24,0.55)]"
          store={viewer.store ?? undefined}
          transport={viewer.transport ?? undefined}
          transportVersion={viewer.transportVersion ?? 0}
          autoConnect={false}
          autoResizeHostOnViewportChange={false}
          showTopBar={false}
          showStatusBar={false}
          hideIdlePlaceholder
          sizingStrategy={rewriteTerminalSizingStrategy}
          sessionId={sessionId ?? undefined}
          showJoinOverlay={false}
          enablePredictiveEcho={false}
          disableViewportMeasurements={disableViewportMeasurements}
          onViewportStateChange={handleViewportStateChange}
        />
      </div>
      {showLoading ? (
        <div className="absolute inset-0 z-10 flex items-center justify-center bg-slate-950/70 text-[13px] font-semibold text-slate-100 backdrop-blur-sm">
          <span>{status === 'connecting' ? 'Connecting to session…' : 'Preparing terminal…'}</span>
        </div>
      ) : null}
      {showError ? (
        <div className="absolute inset-0 z-10 flex items-center justify-center bg-red-500/15 text-[13px] font-semibold text-red-200 backdrop-blur-sm">
          <span>{viewer.error ?? 'Unknown terminal error'}</span>
        </div>
      ) : null}
    </div>
  );
}

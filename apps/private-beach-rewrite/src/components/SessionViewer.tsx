'use client';

import { useCallback, useEffect, useMemo, useState } from 'react';
import type { TerminalViewerState } from '../../../private-beach/src/hooks/terminalViewerTypes';
import { BeachTerminal, type JoinOverlayState, type TerminalViewportState } from '../../../beach-surfer/src/components/BeachTerminal';
import { rewriteTerminalSizingStrategy } from './rewriteTerminalSizing';
import type { TileViewportSnapshot } from '@private-beach-rewrite/features/tiles';

type SessionViewerProps = {
  viewer: TerminalViewerState;
  className?: string;
  tileId?: string | null;
  sessionId?: string | null;
  disableViewportMeasurements?: boolean;
  onViewportMetricsChange?: (snapshot: TileViewportSnapshot | null) => void;
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
  className,
  tileId,
  sessionId,
  disableViewportMeasurements = false,
  onViewportMetricsChange,
}: SessionViewerProps) {
  const status = viewer.status ?? 'idle';
  const showLoading = status === 'idle' || status === 'connecting' || status === 'reconnecting';
  const showError = status === 'error' && Boolean(viewer.error);
  const [joinState, setJoinState] = useState<{ state: JoinOverlayState; message: string | null }>({
    state: 'idle',
    message: null,
  });

  useEffect(() => {
    setJoinState({ state: 'idle', message: null });
  }, [sessionId]);

  const joinOverlay = useMemo(() => {
    if (showError) {
      return null;
    }
    switch (joinState.state) {
      case 'waiting':
        return {
          variant: 'info' as const,
          message: joinState.message ?? 'Waiting for host approval…',
        };
      case 'denied':
        return {
          variant: 'error' as const,
          message: joinState.message ?? 'Join request denied by host.',
        };
      case 'disconnected':
        return {
          variant: 'error' as const,
          message: joinState.message ?? 'Disconnected before host approval.',
        };
      case 'connecting':
      case 'approved':
      case 'idle':
      default:
        return null;
    }
  }, [joinState, showError]);

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
    if (!viewer.store) {
      return undefined;
    }

    const logSnapshot = (reason: string) => {
      try {
        const snap = viewer.store!.getSnapshot();
        const sample = snap.rows
          .slice(0, 20)
          .map((row) =>
            row.kind === 'loaded'
              ? {
                  absolute: row.absolute,
                  text: row.cells.map((cell) => cell.char).join('').trimEnd(),
                }
              : { absolute: row.absolute, text: `[${row.kind}]` },
          );
        // eslint-disable-next-line no-console
        console.info(
          `[rewrite-terminal][snapshot-sample] ${JSON.stringify({
            sessionId,
            reason,
            baseRow: snap.baseRow,
            rows: snap.rows.length,
            sample,
          })}`,
        );
      } catch (error) {
        // eslint-disable-next-line no-console
        console.warn('[rewrite-terminal][snapshot-sample] failed', {
          sessionId,
          reason,
          error,
        });
      }
    };

    logSnapshot('effect-init');
    const unsubscribe = viewer.store.subscribe(() => logSnapshot('store-update'));
    return () => {
      try {
        unsubscribe();
      } catch (error) {
        // eslint-disable-next-line no-console
        console.warn('[rewrite-terminal][snapshot-sample] unsubscribe error', {
          sessionId,
          error,
        });
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

  const resolvedTileId = tileId ?? sessionId ?? 'session-viewer';

  const handleViewportStateChange = useCallback(
    (state: TerminalViewportState) => {
      if (!onViewportMetricsChange) {
        return;
      }
      const snapshot: TileViewportSnapshot = {
        tileId: resolvedTileId,
        hostRows: normalizeMetric(state.hostViewportRows),
        hostCols: normalizeMetric(state.hostCols),
        viewportRows: normalizeMetric(state.viewportRows),
        viewportCols: normalizeMetric(state.viewportCols),
        pixelsPerRow: normalizeMetric(state.pixelsPerRow),
        pixelsPerCol: normalizeMetric(state.pixelsPerCol),
      };
      onViewportMetricsChange(snapshot);
    },
    [onViewportMetricsChange, resolvedTileId],
  );

  useEffect(
    () => () => {
      onViewportMetricsChange?.(null);
    },
    [onViewportMetricsChange],
  );

  return (
    <div className={`session-viewer${className ? ` ${className}` : ''}`} data-status={status}>
      <div
        className="session-viewer__terminal-root"
        data-terminal-root="true"
        data-terminal-tile={resolvedTileId}
      >
        <div className="session-viewer__terminal" data-terminal-content="true">
          <BeachTerminal
            className="session-viewer__terminal"
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
            onJoinStateChange={setJoinState}
            onViewportStateChange={handleViewportStateChange}
          />
        </div>
      </div>
      {showLoading ? (
        <div className="session-viewer__overlay">
          <span>{status === 'connecting' ? 'Connecting to session…' : 'Preparing terminal…'}</span>
        </div>
      ) : null}
      {showError ? (
        <div className="session-viewer__overlay session-viewer__overlay--error">
          <span>{viewer.error ?? 'Unknown terminal error'}</span>
        </div>
      ) : null}
      {!showLoading && !showError && joinOverlay ? (
        <div
          className={`session-viewer__overlay${
            joinOverlay.variant === 'error' ? ' session-viewer__overlay--error' : ''
          }`}
        >
          <span>{joinOverlay.message}</span>
        </div>
      ) : null}
    </div>
  );
}

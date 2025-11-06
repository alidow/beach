'use client';

import { useEffect } from 'react';
import type { TerminalViewerState } from '../../../private-beach/src/hooks/terminalViewerTypes';
import { BeachTerminal } from '../../../beach-surfer/src/components/BeachTerminal';
import { rewriteTerminalSizingStrategy } from './rewriteTerminalSizing';
import { cn } from '@/lib/cn';

type SessionViewerProps = {
  viewer: TerminalViewerState;
  className?: string;
  sessionId?: string | null;
  disableViewportMeasurements?: boolean;
};

export function SessionViewer({ viewer, className, sessionId, disableViewportMeasurements = false }: SessionViewerProps) {
  const status = viewer.status ?? 'idle';
  const showLoading = status === 'idle' || status === 'connecting' || status === 'reconnecting';
  const showError = status === 'error' && Boolean(viewer.error);

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
    const store = viewer.store;
    if (!store || !viewer.transport) {
      return undefined;
    }

    let restoring = false;

    const ensureFollowTail = (reason: string) => {
      if (restoring) {
        return;
      }
      try {
        const snapshot = store.getSnapshot();
        if (!snapshot) {
          return;
        }
        const { rows, viewportHeight, viewportTop, baseRow, followTail } = snapshot;
        let highestLoaded: number | null = null;
        let lowestLoaded: number | null = null;
        let loadedCount = 0;
        for (let index = 0; index < rows.length; index += 1) {
          const row = rows[index];
          if (row && row.kind === 'loaded') {
            loadedCount += 1;
            if (lowestLoaded === null || row.absolute < lowestLoaded) {
              lowestLoaded = row.absolute;
            }
          }
        }
        for (let index = rows.length - 1; index >= 0; index -= 1) {
          const row = rows[index];
          if (row && row.kind === 'loaded') {
            highestLoaded = row.absolute;
            break;
          }
        }
        if (highestLoaded === null || lowestLoaded === null || loadedCount === 0) {
          return;
        }
        const effectiveHeight =
          viewportHeight && viewportHeight > 0 ? viewportHeight : Math.max(1, rows.length || 1);
        const loadedSpan = highestLoaded - lowestLoaded + 1;
        if (loadedSpan < effectiveHeight) {
          return;
        }
        const tailTop = Math.max(baseRow, highestLoaded - (effectiveHeight - 1));
        const needsViewportAdjust = viewportTop !== tailTop || viewportHeight !== effectiveHeight;
        if (followTail && !needsViewportAdjust) {
          return;
        }
        restoring = true;
        if (needsViewportAdjust) {
          store.setViewport(tailTop, effectiveHeight);
        }
        if (store.getSnapshot().followTail) {
          store.setFollowTail(false);
        }
        restoring = false;
        if (process.env.NODE_ENV !== 'production') {
          console.info('[rewrite-terminal-2][follow-tail-restore]', {
            sessionId,
            reason,
            baseRow,
            previousViewportTop: viewportTop,
            previousViewportHeight: viewportHeight,
            rows: rows.length,
            tailTop,
            effectiveHeight,
          });
        }
      } catch (error) {
        restoring = false;
        console.warn('[rewrite-terminal-2][follow-tail-restore] failed', { sessionId, reason, error });
      }
    };

    ensureFollowTail('effect-init');
    const unsubscribe = store.subscribe(() => ensureFollowTail('store-update'));
    return () => {
      try {
        unsubscribe?.();
      } catch (error) {
        console.warn('[rewrite-terminal-2][follow-tail-restore] unsubscribe failed', { sessionId, error });
      }
    };
  }, [sessionId, viewer.store, viewer.transport, viewer.transportVersion]);

  return (
    <div
      className={cn(
        'relative flex h-full min-h-0 w-full flex-1 overflow-hidden rounded-2xl bg-[radial-gradient(circle_at_top,rgba(30,41,59,0.95),rgba(15,23,42,0.92))]',
        className,
      )}
      data-status={status}
    >
      <BeachTerminal
        className="flex h-full w-full flex-1"
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
      />
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

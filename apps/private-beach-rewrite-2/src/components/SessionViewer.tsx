'use client';

import { useCallback, useEffect, useRef } from 'react';
import type { TerminalViewerState } from '../../../private-beach/src/hooks/terminalViewerTypes';
import {
  BeachTerminal,
  type JoinOverlayState,
  type TerminalViewportState,
} from '../../../beach-surfer/src/components/BeachTerminal';
import { rewriteTerminalSizingStrategy } from './rewriteTerminalSizing';
import { cn } from '@/lib/cn';
import type { TileViewportSnapshot } from '@/features/tiles';
import { DragFreezeBoundary } from './DragFreezeBoundary';

type SessionViewerProps = {
  viewer: TerminalViewerState;
  tileId: string;
  className?: string;
  sessionId?: string | null;
  disableViewportMeasurements?: boolean;
  onViewportMetrics?: (snapshot: TileViewportSnapshot | null) => void;
  cellMetrics?: { widthPx: number; heightPx: number };
};

function isTerminalTraceEnabled(): boolean {
  if (typeof globalThis !== 'undefined' && (globalThis as Record<string, any>).__BEACH_TILE_TRACE) {
    return true;
  }
  if (typeof process !== 'undefined' && process.env?.NEXT_PUBLIC_PRIVATE_BEACH_TERMINAL_TRACE === '1') {
    return true;
  }
  return false;
}

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
  disableViewportMeasurements = true,
  onViewportMetrics,
  cellMetrics,
}: SessionViewerProps) {
  const status = viewer.status ?? 'idle';
  const showLoading = status === 'idle' || status === 'connecting' || status === 'reconnecting';
  const showError = status === 'error' && Boolean(viewer.error);
  const metricsRef = useRef<TileViewportSnapshot | null>(null);
  const quantizedCellMetricsRef = useRef<{ width: number | null; height: number | null }>({
    width: null,
    height: null,
  });
  const rootRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (typeof window === 'undefined' || !isTerminalTraceEnabled()) {
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
    if (typeof window === 'undefined' || !isTerminalTraceEnabled()) {
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
        hostWidthPx: normalizeMetric(state.hostPixelWidth),
        hostHeightPx: normalizeMetric(state.hostPixelHeight),
        cellWidthPx: normalizeMetric(state.pixelsPerCol),
        cellHeightPx: normalizeMetric(state.pixelsPerRow),
        quantizedCellWidthPx: normalizeMetric(quantizedCellMetricsRef.current.width),
        quantizedCellHeightPx: normalizeMetric(quantizedCellMetricsRef.current.height),
      };
      const previous = metricsRef.current;
      if (
        previous &&
        previous.hostRows === snapshot.hostRows &&
        previous.hostCols === snapshot.hostCols &&
        previous.viewportRows === snapshot.viewportRows &&
        previous.viewportCols === snapshot.viewportCols &&
        previous.pixelsPerRow === snapshot.pixelsPerRow &&
        previous.pixelsPerCol === snapshot.pixelsPerCol &&
        previous.hostWidthPx === snapshot.hostWidthPx &&
        previous.hostHeightPx === snapshot.hostHeightPx &&
        previous.cellWidthPx === snapshot.cellWidthPx &&
        previous.cellHeightPx === snapshot.cellHeightPx &&
        previous.quantizedCellWidthPx === snapshot.quantizedCellWidthPx &&
        previous.quantizedCellHeightPx === snapshot.quantizedCellHeightPx
      ) {
        return;
      }
      metricsRef.current = snapshot;
      onViewportMetrics(snapshot);
    },
    [onViewportMetrics, tileId],
  );

  const handleJoinStateChange = useCallback(
    (_snapshot: { state: JoinOverlayState; message: string | null }) => {
      // Placeholder to keep hook usage inside component scope.
    },
    [],
  );

  // Quantize terminal cell width to the device-pixel grid (and canvas scale)
  // to reduce horizontal drift from subpixel widths.
  // Guard: set window.__BEACH_DISABLE_CELL_QUANTIZE = true to disable.
  useEffect(() => {
    if (typeof window === 'undefined') return;
    if ((globalThis as Record<string, any>).__BEACH_DISABLE_CELL_QUANTIZE) return;

    const root = rootRef.current;
    if (!root) return;

    const terminalEl = root.querySelector<HTMLElement>('.beach-terminal');
    if (!terminalEl) return;

    const dpr = Math.max(1, window.devicePixelRatio || 1);
    const originalVar = terminalEl.style.getPropertyValue('--beach-terminal-cell-width');

    let raf = 0;
    let ro: ResizeObserver | null = null;

    const applyQuantize = () => {
      try {
        const computed = window.getComputedStyle(terminalEl);
        let cssVal = computed.getPropertyValue('--beach-terminal-cell-width');
        if (!cssVal || !cssVal.trim()) cssVal = originalVar;
        const px = parseFloat(cssVal);
        if (!Number.isFinite(px) || px <= 0) return;
        const { scale } = readTransformChain(terminalEl);
        const denom = dpr * (Number.isFinite(scale) && scale > 0 ? scale : 1);
        const quantized = Math.max(0.25, Math.round(px * denom) / denom);
        const delta = Math.abs(quantized - px);
        if (delta > 0.002) {
          terminalEl.style.setProperty('--beach-terminal-cell-width', `${quantized.toFixed(3)}px`);
        }
        quantizedCellMetricsRef.current.width = quantized;
        quantizedCellMetricsRef.current.height = null;
        const traceEnabled = isTerminalTraceEnabled();
        if (traceEnabled) {
          try {
            console.info('[rewrite-terminal][tile-trace]', JSON.stringify({
              reason: 'quantize',
              tileId,
              cssCellWidthPx: Number.isFinite(px) ? px : null,
              quantizedCellWidthPx: quantized,
              delta,
              scale,
              denom,
            }));
          } catch {
            // ignore logging errors
          }
        }
      } catch {
        // ignore
      }
    };

    const schedule = () => {
      if (raf) cancelAnimationFrame(raf);
      raf = requestAnimationFrame(applyQuantize);
    };

    schedule();
    if ('ResizeObserver' in window) {
      ro = new ResizeObserver(() => schedule());
      ro.observe(terminalEl);
      ro.observe(root);
    }
    window.addEventListener('resize', schedule);

    return () => {
      if (raf) cancelAnimationFrame(raf);
      if (ro) ro.disconnect();
      window.removeEventListener('resize', schedule);
      try {
        if (originalVar) {
          terminalEl.style.setProperty('--beach-terminal-cell-width', originalVar);
        } else {
          terminalEl.style.removeProperty('--beach-terminal-cell-width');
        }
      } catch {}
    };
  }, []);

  // Pixel-snap terminal content to the device pixel grid to avoid
  // subpixel misalignment when the canvas pan/translate is fractional.
  // Guarded by a global opt-out flag: set window.__BEACH_DISABLE_PIXEL_SNAP = true
  // to disable without code changes.
  useEffect(() => {
    if (typeof window === 'undefined') return;
    if ((globalThis as Record<string, any>).__BEACH_DISABLE_PIXEL_SNAP) return;

    const root = rootRef.current;
    if (!root) return;
    const content = root.querySelector<HTMLElement>('[data-terminal-content="true"]');
    if (!content) return;

    let raf = 0;
    let ro: ResizeObserver | null = null;
    const initialTransform = content.style.transform && content.style.transform !== 'none'
      ? content.style.transform
      : '';

    const applySnap = () => {
      try {
        const rect = content.getBoundingClientRect();
        const dpr = Math.max(1, window.devicePixelRatio || 1);
        const leftDevice = rect.left * dpr;
        const frac = leftDevice - Math.round(leftDevice);
        const offsetDevice = Math.abs(frac) < 0.01 ? 0 : -frac;
        const offsetCss = offsetDevice / dpr;
        const base = initialTransform;
        const snap = `translateX(${offsetCss.toFixed(3)}px)`;
        content.style.willChange = 'transform';
        content.style.transform = base ? `${base} ${snap}` : snap;
      } catch {
        // ignore
      }
    };

    const schedule = () => {
      if (raf) cancelAnimationFrame(raf);
      raf = requestAnimationFrame(applySnap);
    };

    schedule();
    if ('ResizeObserver' in window) {
      ro = new ResizeObserver(() => schedule());
      ro.observe(root);
      const contentEl = content as Element;
      ro.observe(contentEl);
    }
    window.addEventListener('resize', schedule);
    return () => {
      if (raf) cancelAnimationFrame(raf);
      if (ro) ro.disconnect();
      window.removeEventListener('resize', schedule);
      try {
        content.style.transform = initialTransform;
        if (!initialTransform) content.style.removeProperty('transform');
        content.style.willChange = '';
      } catch {}
    };
  }, []);

  // Verbose instrumentation for diagnosing alignment issues.
  useEffect(() => {
    if (typeof window === 'undefined' || !isTerminalTraceEnabled()) return;
    const root = rootRef.current;
    if (!root) return;
    const terminalEl = root.querySelector<HTMLElement>('.beach-terminal');
    if (!terminalEl) return;

    const logSnapshot = (reason: string) => {
      try {
        const computed = window.getComputedStyle(terminalEl);
        const cssVar = computed.getPropertyValue('--beach-terminal-cell-width');
        const cssPx = parseFloat(cssVar);
        const letterSpacing = computed.letterSpacing;
        const fontFamily = computed.fontFamily;
        const fontSize = computed.fontSize;
        const span = terminalEl.querySelector<HTMLSpanElement>('.xterm-row span');
        const spanWidth = span ? span.getBoundingClientRect().width : null;
        const row = terminalEl.querySelector('.xterm-row');
        const renderedCells = row ? row.querySelectorAll('span').length : null;
        const { scale: flowScale, translateX, translateY } = readTransformChain(root);
        const { scale: terminalScale, translateX: terminalTranslateX, translateY: terminalTranslateY } =
          readTransformChain(terminalEl);
        const metrics = metricsRef.current;
        // eslint-disable-next-line no-console
        console.info('[rewrite-terminal][tile-trace]', JSON.stringify({
          reason,
          tileId,
          cssCellWidthPx: Number.isFinite(cssPx) ? cssPx : null,
          spanWidthPx: spanWidth,
          letterSpacing,
          fontFamily,
          fontSize,
          flowScale,
          flowTranslateX: translateX,
          flowTranslateY: translateY,
          terminalScale,
          terminalTranslateX,
          terminalTranslateY,
          dpr: window.devicePixelRatio || 1,
          renderedCells,
          metrics,
        }));
      } catch (error) {
        // eslint-disable-next-line no-console
        console.warn('[rewrite-terminal][tile-trace] error', error);
      }
    };

    logSnapshot('initial');
    const interval = window.setInterval(() => logSnapshot('interval'), 1500);
    return () => {
      window.clearInterval(interval);
    };
  }, [tileId]);

  return (
    <div
      ref={rootRef}
      className={cn('relative flex h-full min-h-0 w-full flex-1 overflow-hidden', className)}
      data-status={status}
    >
      <div
        className="flex h-full w-full flex-1"
        data-terminal-root="true"
        data-terminal-tile={tileId}
      >
        <div className="flex h-full w-full flex-1 overflow-hidden" data-terminal-content="true">
          <DragFreezeBoundary>
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
              lockViewportToHost
              cellMetrics={cellMetrics}
              onViewportStateChange={handleViewportStateChange}
              onJoinStateChange={handleJoinStateChange}
            />
          </DragFreezeBoundary>
        </div>
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

function readTransformChain(element: Element | null): { scale: number; translateX: number; translateY: number } {
  if (typeof window === 'undefined') {
    return { scale: 1, translateX: 0, translateY: 0 };
  }
  let current: Element | null = element;
  while (current) {
    const computed = window.getComputedStyle(current);
    const raw = computed.transform || (computed as any).webkitTransform || '';
    const parsed = parseTransformMatrix(raw);
    if (parsed) {
      return parsed;
    }
    current = current.parentElement;
  }
  return { scale: 1, translateX: 0, translateY: 0 };
}

function parseTransformMatrix(value: string): { scale: number; translateX: number; translateY: number } | null {
  if (!value || value === 'none') {
    return null;
  }
  const matrix2d = value.match(/matrix\(([-0-9.,\s]+)\)/);
  if (matrix2d && matrix2d[1]) {
    const parts = matrix2d[1]
      .split(',')
      .map((segment) => parseFloat(segment.trim()));
    if (parts.length >= 6 && parts.every((num) => Number.isFinite(num))) {
      const [a, b, , , tx, ty] = parts;
      const scale = Math.sqrt(a * a + b * b) || 1;
      return {
        scale,
        translateX: Number.isFinite(tx) ? tx : 0,
        translateY: Number.isFinite(ty) ? ty : 0,
      };
    }
  }
  const matrix3d = value.match(/matrix3d\(([-0-9.,\s]+)\)/);
  if (matrix3d && matrix3d[1]) {
    const parts = matrix3d[1]
      .split(',')
      .map((segment) => parseFloat(segment.trim()));
    if (parts.length >= 16 && parts.every((num) => Number.isFinite(num))) {
      const a = parts[0];
      const b = parts[1];
      const tx = parts[12];
      const ty = parts[13];
      const scale = Math.sqrt(a * a + b * b) || 1;
      return {
        scale,
        translateX: Number.isFinite(tx) ? tx : 0,
        translateY: Number.isFinite(ty) ? ty : 0,
      };
    }
  }
  return null;
}
